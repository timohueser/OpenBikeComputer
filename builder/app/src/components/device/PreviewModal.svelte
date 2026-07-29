<!--
  The "chart room" preview (#894 epic, preview redesign of 2026-07-29; the wireframe's Option 1):
  one near-fullscreen surface for routes, rides and trips — stats as a chip strip in the header,
  the map filling everything, waypoints floating on it, and a zoomable elevation profile along the
  bottom whose window the map echoes (the windowed span stays coral, the rest drops to gray-green).

  A plain overlay, not a `<dialog>` — same WebKitGTK reasoning as `ConfirmDialog.svelte`. The map
  is a second, short-lived Leaflet instance: created when the modal mounts, `invalidateSize`d on
  the next frame (the container has no size until layout has run), removed on teardown. Leaflet's
  CSS is already global (`main.ts`), and this component only ever lives inside the lazily-loaded
  device/rides chunks, so the static `import L` adds nothing to the entry bundle it wasn't
  already carrying via the region picker.

  The zoom state is one `[t0, t1]` window over distance-along-track (`lib/device/elevation`), and
  every surface reads it: the profile redraws the windowed span from the real points, the map's
  coral polyline is re-sliced to `windowIndexRange`, and the km caption names the span. Drag on
  the profile selects a range (within the current window), Ctrl/⌘-drag pans a zoomed window along
  the track, wheel zooms about the cursor, double-click resets. A preview without drawable
  elevation (trips, elevation-less rides) has no profile strip and no zoom — the map stays fully
  coral. The map deliberately does NOT re-fit on zoom changes: one initial fitBounds, then the
  viewport is the rider's.

  Hover is synced both ways over the same distance axis: a pointer on the profile draws a thin
  cursor line there and an amber dot on the map at the matching point along the track
  (`pointAtDistance`); a pointer on either map polyline snaps the dot to the nearest track point
  (`nearestPointIndex`) and puts the cursor line at its distance. One lazily-created circleMarker,
  mutated with `setLatLng`, updates gated through rAF — no layer churn on mousemove.
-->
<script lang="ts">
    import L from "leaflet";
    import type { RouteWaypoint } from "../../lib/convert/bridge";
    import {
        FULL_WINDOW,
        clampWindow,
        cumulativeDistances,
        elevationProfile,
        isFullWindow,
        nearestPointIndex,
        panWindow,
        pointAtDistance,
        windowIndexRange,
        zoomWindow,
        type ProfilePoint,
        type ProfileWindow,
    } from "../../lib/device/elevation";

    const PROFILE = { width: 900, height: 96, pad: 2 };
    /** A drag narrower than this fraction of the strip is a click, not a range. */
    const MIN_DRAG_FRACTION = 0.005;
    /** Wheel-to-zoom gearing: one notch (~100 deltaY) scales the window by e^0.2 ≈ 1.22. */
    const WHEEL_GEARING = 0.002;

    let {
        title,
        points,
        stats,
        onclose,
        actions,
        waypoints = [],
    }: {
        title: string;
        points: readonly ProfilePoint[];
        /** Label/value pairs for the header's stat chips, already formatted. */
        stats: ReadonlyArray<{ label: string; value: string }>;
        onclose: () => void;
        /** The header's action side — Delete, Pull, whatever the object supports. */
        actions?: import("svelte").Snippet;
        /** The route's stored waypoints (OBCR §4); empty for rides and trips. */
        waypoints?: readonly RouteWaypoint[];
    } = $props();

    let mapEl = $state<HTMLDivElement>();
    let profileEl = $state<HTMLDivElement>();
    let closeButton = $state<HTMLButtonElement>();
    let wpRows: HTMLButtonElement[] = [];

    let win = $state<ProfileWindow>(FULL_WINDOW);
    /** An in-progress drag on the profile, as fractions of the strip's width: a plain drag selects
     *  a range, a Ctrl/⌘ drag pans the (zoomed) window from its position at pointer-down. */
    let drag = $state<
        | { mode: "select"; from: number; to: number }
        | { mode: "pan"; from: number; base: ProfileWindow }
        | null
    >(null);
    let selectedWp = $state<number | null>(null);

    /** The hovered position along the whole track, as a fraction of the total distance — fed by
     *  both the profile strip and the map polylines, drawn by both. Null: nothing hovered. */
    let hoverT = $state<number | null>(null);

    const cum = $derived(cumulativeDistances(points));
    /** The full-track profile decides whether the strip (and the whole zoom UI) exists at all. */
    const hasProfile = $derived(elevationProfile(points, PROFILE.width, PROFILE.height) !== null);
    const profile = $derived(
        hasProfile ? elevationProfile(points, PROFILE.width, PROFILE.height, win) : null,
    );

    /** The windowed polyline, owned by the map effect, re-sliced by the window effect. */
    let coralLine = $state<L.Polyline | null>(null);
    let leafletMap: L.Map | null = null;
    let markers: L.Marker[] = [];

    // --- the shared hover cursor -----------------------------------------------------------
    //
    // One rAF gate for both sources (profile pointermove, map polyline mousemove): the latest
    // report wins, `hoverT` changes at most once a frame, and everything downstream — the strip's
    // cursor line, the map dot's `setLatLng` — hangs off that one state. A source may queue a
    // thunk instead of a value, in which case its work (the map side's nearest-point scan) also
    // runs at most once a frame, not once per raw mousemove.

    let hoverRaf = 0;
    let hoverNext: number | null | (() => number | null) = null;
    function queueHover(t: number | null | (() => number | null)) {
        hoverNext = t;
        if (hoverRaf) return;
        hoverRaf = requestAnimationFrame(() => {
            hoverRaf = 0;
            hoverT = typeof hoverNext === "function" ? hoverNext() : hoverNext;
        });
    }

    /** The map dot: created on the first hover, then only ever moved / attached / detached. */
    let hoverMarker: L.CircleMarker | null = null;
    let hoverShown = false;

    $effect(() => {
        const t = hoverT;
        const map = leafletMap;
        if (!map) return;
        if (t === null) {
            if (hoverShown) {
                hoverMarker?.remove();
                hoverShown = false;
            }
            return;
        }
        const at = pointAtDistance(points, cum, t * totalTrackM);
        if (!at) return;
        // Amber with a panel-colored ring — visible on the coral span and the gray remainder
        // alike. Non-interactive so it never steals the mousemove from the line under it.
        hoverMarker ??= L.circleMarker([at.lat, at.lon], {
            radius: 5.5,
            weight: 2,
            color: "#f3f0df",
            fillColor: "#e3ad33",
            fillOpacity: 1,
            interactive: false,
        });
        hoverMarker.setLatLng([at.lat, at.lon]);
        if (!hoverShown) {
            hoverMarker.addTo(map);
            hoverShown = true;
        }
    });

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
            // The subdued whole track underneath, the windowed span in coral on top. At the full
            // window the coral covers the gray entirely — today's all-coral look.
            const gray = L.polyline(latlngs, { color: "#8b957f", weight: 4 });
            gray.addTo(map);
            const coral = L.polyline(latlngs, { color: "#cf6a2a", weight: 5 });
            coral.addTo(map);
            if (hasProfile) {
                // The reverse hover: a pointer on either polyline snaps the dot to the nearest
                // track point and the profile draws its cursor line at that distance. Without a
                // profile there is nothing to sync, so the map stays hover-quiet.
                const report = (event: L.LeafletMouseEvent) => {
                    // Queued as a thunk: the O(n) scan runs inside the rAF gate, once a frame.
                    const { lat, lng } = event.latlng;
                    queueHover(() => {
                        const i = nearestPointIndex(points, lat, lng);
                        return i >= 0 && totalTrackM > 0 ? cum[i] / totalTrackM : null;
                    });
                };
                const clear = () => queueHover(null);
                for (const line of [gray, coral]) {
                    line.on("mousemove", report);
                    line.on("mouseout", clear);
                }
            }
            L.circleMarker(latlngs[0], { radius: 5, color: "#3c6b39", fillColor: "#3c6b39", fillOpacity: 1 }).addTo(map);
            L.circleMarker(latlngs[latlngs.length - 1], { radius: 5, color: "#3c6b39", fillOpacity: 0 }).addTo(map);
            map.fitBounds(gray.getBounds(), { padding: [24, 24] });
            coralLine = coral;
        } else {
            map.setView(latlngs[0] ?? [0, 0], latlngs.length ? 13 : 2);
        }

        markers = waypoints.map((w, i) => {
            const marker = L.marker([w.lat, w.lon], {
                icon: L.divIcon({ className: "wpanchor", html: '<div class="wpdiamond"></div>', iconSize: [13, 13] }),
                title: w.name,
            });
            // `textContent`, never an HTML string: the name is rider data, not markup.
            marker.bindPopup(() => {
                const el = document.createElement("div");
                el.className = "wppopup";
                el.textContent = w.name || "Waypoint";
                return el;
            });
            marker.on("click", () => {
                selectedWp = i;
                wpRows[i]?.scrollIntoView({ block: "nearest" });
            });
            marker.addTo(map);
            return marker;
        });

        leafletMap = map;
        // The container was laid out after `L.map` measured it; recheck on the next frame.
        const raf = requestAnimationFrame(() => map.invalidateSize());
        return () => {
            cancelAnimationFrame(raf);
            if (hoverRaf) {
                cancelAnimationFrame(hoverRaf);
                hoverRaf = 0;
            }
            // The dot belonged to this map instance; the next one starts without.
            hoverMarker = null;
            hoverShown = false;
            coralLine = null;
            leafletMap = null;
            markers = [];
            map.remove();
        };
    });

    // The map's echo of the profile window: re-slice the coral polyline, nothing else — no
    // re-fit, no recreation. The gray full track only shows once a window exists.
    $effect(() => {
        const coral = coralLine;
        if (!coral) return;
        const [first, last] = windowIndexRange(cum, win);
        coral.setLatLngs(points.slice(first, last + 1).map((p) => [p.lat, p.lon] as [number, number]));
    });

    // A new object in the same modal instance starts back at the whole track, nothing hovered.
    $effect(() => {
        void points;
        win = FULL_WINDOW;
        selectedWp = null;
        hoverT = null;
    });

    // Wheel-to-zoom needs `preventDefault` (the page must not scroll behind it), so the listener
    // is attached by hand with `passive: false` rather than through a template handler.
    $effect(() => {
        const el = profileEl;
        if (!el) return;
        const onWheel = (event: WheelEvent) => {
            event.preventDefault();
            const t = win[0] + fractionAt(el, event.clientX) * (win[1] - win[0]);
            win = zoomWindow(win, Math.exp(event.deltaY * WHEEL_GEARING), t);
        };
        el.addEventListener("wheel", onWheel, { passive: false });
        return () => el.removeEventListener("wheel", onWheel);
    });

    function fractionAt(el: HTMLElement, clientX: number): number {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 ? Math.max(0, Math.min(1, (clientX - rect.left) / rect.width)) : 0;
    }

    function onProfileDown(event: PointerEvent) {
        if (!profileEl || event.button !== 0) return;
        profileEl.setPointerCapture(event.pointerId);
        const f = fractionAt(profileEl, event.clientX);
        // Ctrl (or ⌘) turns the drag into a pan of the zoomed window; at the full window there is
        // nothing to pan, so the modifier is simply ignored there.
        drag =
            (event.ctrlKey || event.metaKey) && !isFullWindow(win)
                ? { mode: "pan", from: f, base: win }
                : { mode: "select", from: f, to: f };
    }

    function onProfileMove(event: PointerEvent) {
        if (!profileEl) return;
        const f = fractionAt(profileEl, event.clientX);
        // Hover tracks the pointer whether or not a drag is running — the cursor line is where
        // the pointer is either way.
        queueHover(win[0] + f * (win[1] - win[0]));
        if (!drag) return;
        if (drag.mode === "pan") {
            // Content follows the pointer: a strip fraction moved right slides the window left by
            // the same share of the window it was when the pan began.
            win = panWindow(drag.base, -(f - drag.from) * (drag.base[1] - drag.base[0]));
        } else {
            drag = { ...drag, to: f };
        }
    }

    function onProfileUp() {
        const d = drag;
        drag = null;
        // A pan applied itself live; only a wide-enough select commits a new window here.
        if (!d || d.mode !== "select" || Math.abs(d.to - d.from) < MIN_DRAG_FRACTION) return;
        // The drag selects within the *current* window: fractions of the strip map back onto it.
        const span = win[1] - win[0];
        win = clampWindow(win[0] + d.from * span, win[0] + d.to * span);
    }

    function resetZoom() {
        drag = null;
        win = FULL_WINDOW;
    }

    function focusWaypoint(i: number) {
        selectedWp = i;
        const w = waypoints[i];
        leafletMap?.panTo([w.lat, w.lon]);
        markers[i]?.openPopup();
    }

    const totalTrackM = $derived(cum.length ? cum[cum.length - 1] : 0);

    /** The hover cursor's x across the strip, in percent — null when nothing is hovered or the
     *  hovered distance falls outside the current window (a map hover on the gray remainder). */
    const hoverPct = $derived.by(() => {
        if (hoverT === null || !profile) return null;
        const d = hoverT * totalTrackM;
        const span = profile.endM - profile.startM;
        if (span <= 0 || d < profile.startM || d > profile.endM) return null;
        return ((d - profile.startM) / span) * 100;
    });

    /**
     * A waypoint's distance clamped onto the drawn axis. `distAlongM` was measured on the RAW
     * pre-decimation track at pack time, so it can slightly exceed the decimated read-back
     * polyline's total — an end-of-route waypoint must not fall off the strip (or read farther
     * than the caption's total) over that difference.
     */
    function clampedDistM(w: RouteWaypoint): number {
        return totalTrackM > 0 ? Math.max(0, Math.min(w.distAlongM, totalTrackM)) : w.distAlongM;
    }

    /** A waypoint's x position across the profile strip for the current window, in percent —
     *  null only when it falls outside the current zoom window. */
    function tickPercent(w: RouteWaypoint): number | null {
        if (!profile) return null;
        const span = profile.endM - profile.startM;
        if (span <= 0) return null;
        const d = clampedDistM(w);
        if (d < profile.startM || d > profile.endM) return null;
        return ((d - profile.startM) / span) * 100;
    }

    const km = (m: number) => (m / 1000).toFixed(1);

    function onKeydown(event: KeyboardEvent) {
        if (event.key === "Escape") {
            event.preventDefault();
            onclose();
        }
    }
</script>

<svelte:window on:keydown={onKeydown} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
    <div class="sheet" role="dialog" aria-modal="true" aria-labelledby="preview-title">
        <div class="head">
            <h3 id="preview-title">{title}</h3>
            <div class="chips">
                {#each stats as stat (stat.label)}
                    <span class="statchip">
                        <span class="l">{stat.label}</span>
                        <span class="v">{stat.value}</span>
                    </span>
                {/each}
                {#if waypoints.length > 0}
                    <span class="statchip">
                        <span class="l">Waypoints</span>
                        <span class="v">{waypoints.length}</span>
                    </span>
                {/if}
            </div>
            <div class="headend">
                {@render actions?.()}
                <button type="button" class="iconbtn" bind:this={closeButton} onclick={onclose} aria-label="Close">
                    ✕
                </button>
            </div>
        </div>

        <div class="map" bind:this={mapEl}>
            {#if waypoints.length > 0}
                <div class="wplist">
                    <p class="wh small faint">Waypoints · {waypoints.length}</p>
                    <div class="wprows">
                        {#each waypoints as w, i (i)}
                            <button
                                type="button"
                                class="wprow"
                                class:sel={selectedWp === i}
                                bind:this={wpRows[i]}
                                onclick={() => focusWaypoint(i)}
                            >
                                <span class="wpglyph" aria-hidden="true"></span>
                                <span class="wpname">{w.name || "Waypoint"}</span>
                                <span class="wpkm small faint">km {km(clampedDistM(w))}</span>
                            </button>
                        {/each}
                    </div>
                </div>
            {/if}
        </div>

        {#if hasProfile}
            <!-- Pointer-driven zoom control; role=application keeps the a11y tree honest about it. -->
            <div
                class="profile"
                role="application"
                aria-label="Elevation profile — drag or scroll to zoom, ctrl-drag to pan, double-click to reset"
                bind:this={profileEl}
                onpointerdown={onProfileDown}
                onpointermove={onProfileMove}
                onpointerup={onProfileUp}
                onpointerleave={() => queueHover(null)}
                onpointercancel={() => (drag = null)}
                ondblclick={resetZoom}
            >
                {#if profile}
                    <svg
                        viewBox="0 0 {PROFILE.width} {PROFILE.height}"
                        preserveAspectRatio="none"
                        aria-label="Elevation profile"
                        role="img"
                    >
                        <path class="area" d={profile.areaPath} />
                        <path class="line" d={profile.linePath} />
                    </svg>
                {/if}
                {#each waypoints as w, i (i)}
                    {@const pct = tickPercent(w)}
                    {#if pct !== null}
                        <span class="wptick" style="left: {pct}%" title={w.name}></span>
                    {/if}
                {/each}
                {#if drag && drag.mode === "select" && Math.abs(drag.to - drag.from) >= MIN_DRAG_FRACTION}
                    <div
                        class="band"
                        style="left: {Math.min(drag.from, drag.to) * 100}%; width: {Math.abs(drag.to - drag.from) * 100}%"
                    ></div>
                {/if}
                {#if hoverPct !== null}
                    <div class="cursorline" style="left: {hoverPct}%"></div>
                {/if}
                {#if profile}
                    <p class="caption small faint">
                        Elevation · {Math.round(profile.minEle).toLocaleString()} –
                        {Math.round(profile.maxEle).toLocaleString()} m
                        {#if !isFullWindow(win)}
                            · km {km(profile.startM)} – {km(profile.endM)} of {km(profile.totalM)}
                        {:else}
                            · {km(profile.totalM)} km
                        {/if}
                    </p>
                {:else}
                    <p class="caption small faint">No elevation in this span — double-click to reset.</p>
                {/if}
                <p class="hint small faint">drag or scroll to zoom · ctrl/⌘-drag pans · double-click resets</p>
            </div>
        {/if}
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
        padding: 16px;
    }

    .sheet {
        width: min(94vw, 1500px);
        height: min(90vh, 980px);
        display: flex;
        flex-direction: column;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 16px;
        overflow: hidden;
        box-shadow: 0 18px 44px rgba(32, 48, 29, 0.28);
    }

    .head {
        display: flex;
        align-items: center;
        gap: 14px;
        padding: 10px 16px;
        border-bottom: 1px solid var(--line);
        flex-wrap: wrap;
    }

    h3 {
        font-family: var(--serif);
        font-size: 18px;
        margin: 0;
        min-width: 0;
    }

    .chips {
        display: flex;
        align-items: center;
        min-width: 0;
        overflow-x: auto;
    }

    .statchip {
        display: inline-flex;
        flex-direction: column;
        line-height: 1.25;
        padding: 0 12px;
        border-left: 1px solid var(--line);
        white-space: nowrap;
    }

    .statchip:first-child {
        border-left: 0;
    }

    .statchip .l {
        font-size: 9.5px;
        letter-spacing: 0.06em;
        text-transform: uppercase;
        color: var(--ink-faint);
    }

    .statchip .v {
        font-size: 14px;
        font-weight: 650;
        font-variant-numeric: tabular-nums;
    }

    .headend {
        margin-left: auto;
        display: flex;
        gap: 8px;
        align-items: center;
    }

    .map {
        flex: 1;
        min-height: 0;
        /* Leaflet panes sit at z-index up to 700 — keep them inside this box's
           stacking context so tiles never paint over the modal chrome. */
        position: relative;
        isolation: isolate;
    }

    /* --- waypoints: the floating card, the map diamonds --- */

    .wplist {
        position: absolute;
        top: 12px;
        right: 12px;
        z-index: 800; /* over Leaflet's panes, inside the map's stacking context */
        width: 235px;
        max-height: min(46%, 300px);
        display: flex;
        flex-direction: column;
        background: color-mix(in srgb, var(--panel) 92%, transparent);
        border: 1px solid var(--parchment-3);
        border-radius: 10px;
        overflow: hidden;
        box-shadow: 0 2px 8px rgba(36, 51, 28, 0.12);
    }

    .wh {
        margin: 0;
        padding: 7px 10px 5px;
        letter-spacing: 0.06em;
        text-transform: uppercase;
        font-size: 10.5px;
        border-bottom: 1px solid var(--line);
    }

    .wprows {
        overflow-y: auto;
    }

    .wprow {
        display: flex;
        align-items: center;
        gap: 8px;
        width: 100%;
        padding: 5px 10px;
        font-size: 12.5px;
        font-family: var(--sans);
        color: var(--ink);
        text-align: left;
        background: transparent;
        border: 0;
    }

    .wprow:hover,
    .wprow.sel {
        background: var(--parchment-2);
    }

    .wpglyph {
        width: 9px;
        height: 9px;
        flex: none;
        transform: rotate(45deg);
        background: var(--water);
        border: 1.5px solid var(--panel);
        box-shadow: 0 0 0 1px var(--water);
    }

    .wpname {
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .wpkm {
        font-variant-numeric: tabular-nums;
        flex: none;
    }

    /* The Leaflet marker DOM is injected, so its styles must escape the component scope. */
    :global(.wpanchor) {
        background: transparent;
        border: 0;
    }

    :global(.wpanchor .wpdiamond) {
        width: 11px;
        height: 11px;
        margin: 1px;
        transform: rotate(45deg);
        background: var(--water);
        border: 2px solid var(--panel);
        box-shadow: 0 0 0 1px var(--water);
    }

    :global(.wppopup) {
        font-family: var(--sans);
        font-size: 13px;
    }

    /* --- the zoomable profile strip --- */

    .profile {
        position: relative;
        height: 120px;
        flex: none;
        border-top: 1px solid var(--line);
        background: var(--panel);
        cursor: crosshair;
        touch-action: none; /* the strip owns its pointer gestures */
        user-select: none;
    }

    .profile svg {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
        display: block;
    }

    .profile .area {
        fill: var(--wood);
        fill-opacity: 0.25;
    }

    .profile .line {
        fill: none;
        stroke: var(--forest);
        stroke-width: 2;
    }

    .band {
        position: absolute;
        top: 0;
        bottom: 0;
        background: rgba(207, 106, 42, 0.14);
        border-left: 1.5px dashed var(--coral);
        border-right: 1.5px dashed var(--coral);
        pointer-events: none;
    }

    .cursorline {
        position: absolute;
        top: 0;
        bottom: 0;
        border-left: 1px solid var(--ink-soft);
        opacity: 0.65;
        pointer-events: none;
    }

    .wptick {
        position: absolute;
        bottom: 10px;
        width: 7px;
        height: 7px;
        margin-left: -3.5px;
        transform: rotate(45deg);
        background: var(--water);
        pointer-events: none;
    }

    .caption {
        position: absolute;
        left: 12px;
        top: 6px;
        margin: 0;
        pointer-events: none;
    }

    .hint {
        position: absolute;
        right: 12px;
        top: 6px;
        margin: 0;
        pointer-events: none;
        opacity: 0.8;
    }

    @media (max-width: 700px) {
        .backdrop {
            padding: 8px;
        }

        .sheet {
            width: 100%;
            height: 94vh;
        }

        .hint {
            display: none;
        }
    }
</style>
