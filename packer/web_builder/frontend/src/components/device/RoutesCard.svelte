<!--
  Routes & trips, as the card holds them: trips render as groups of their stage routes — the same
  grouping the device's own UI shows — and every route outside a trip is a top-level row.

  Facts come straight off the list entries; the one editorial addition is the expiry column,
  which the wire has always carried (`expiresAt`/`retention`) and no UI has ever shown.
-->
<script lang="ts">
    import { dashboard, type TripView } from "../../lib/device/dashboard.svelte";
    import type { RouteListEntry } from "../../lib/usb/objects";
    import { formatBytes } from "../../lib/format";

    let {
        ondelete,
        ondeletetrip,
        row,
        tripbar,
    }: {
        /** Delete a top-level route. Wired by the page so the card stays cable-free. */
        ondelete: (route: RouteListEntry) => void;
        ondeletetrip: (trip: TripView) => void;
        /** Extra per-route actions (preview, menu) — the page grows these in later steps. */
        row?: import("svelte").Snippet<[RouteListEntry, TripView | null]>;
        /** Extra per-trip actions rendered in the group bar. */
        tripbar?: import("svelte").Snippet<[TripView]>;
    } = $props();

    function facts(route: RouteListEntry): string {
        const parts = [
            `${(route.distanceM / 1000).toFixed(1)} km`,
            `${route.ascentM.toLocaleString()} m up`,
            `${route.pointCount.toLocaleString()} points`,
        ];
        if (route.waypointCount > 0)
            parts.push(`${route.waypointCount} waypoint${route.waypointCount === 1 ? "" : "s"}`);
        parts.push(formatBytes(route.byteLen));
        return parts.join(" · ");
    }

    /** What the retention clock means for this route, in one short phrase. */
    function expiry(route: RouteListEntry): string {
        if (route.retention === 0) return "kept forever";
        if (route.expiresAt === 0) return "expiry not started";
        const days = Math.ceil((route.expiresAt * 1000 - Date.now()) / 86_400_000);
        if (days <= 0) return "expiring";
        return days === 1 ? "expires tomorrow" : `expires in ${days} days`;
    }

    function tripFacts(trip: TripView): string {
        return `${(trip.totalDistanceM / 1000).toFixed(1)} km · ${trip.totalAscentM.toLocaleString()} m up in total`;
    }
</script>

<section class="card">
    <div class="sechead">
        <h3>Routes &amp; trips</h3>
        <span class="small faint">
            {dashboard.routes.length}
            {dashboard.routes.length === 1 ? "route" : "routes"}{#if dashboard.trips.length}
                · {dashboard.trips.length}
                {dashboard.trips.length === 1 ? "trip" : "trips"}{/if}
        </span>
    </div>

    {#each dashboard.trips as trip (trip.objectId)}
        <div class="tripbox">
            <div class="tripbar">
                <p class="name">{trip.name || `Trip ${trip.objectId}`}</p>
                <span class="tag">trip</span>
                <p class="small faint grow">{tripFacts(trip)}</p>
                {@render tripbar?.(trip)}
                <button type="button" class="btn ghost" onclick={() => ondeletetrip(trip)}>
                    Delete
                </button>
            </div>
            {#if trip.detail === null}
                <p class="small faint pad">The trip's stage list could not be read.</p>
            {:else}
                <ul class="rows inset">
                    {#each dashboard.stagesOf(trip) as stage, index (`${stage.id}-${index}`)}
                        <li>
                            {#if stage.route}
                                <span class="grow">
                                    <p class="name">{stage.route.name || `Route ${stage.id}`}</p>
                                    <p class="small faint">{facts(stage.route)}</p>
                                </span>
                                {@render row?.(stage.route, trip)}
                            {:else}
                                <span class="grow">
                                    <p class="name faint">Route {stage.id}</p>
                                    <p class="small faint">no longer on the device</p>
                                </span>
                            {/if}
                        </li>
                    {/each}
                </ul>
            {/if}
        </div>
    {/each}

    {#if dashboard.topLevelRoutes.length === 0 && dashboard.trips.length === 0}
        <p class="small muted">
            No routes on the device yet. Drop a GPX below — it is converted here and written to the
            card.
        </p>
    {:else}
        <ul class="rows">
            {#each dashboard.topLevelRoutes as route (route.objectId)}
                <li>
                    <span class="grow">
                        <p class="name">{route.name || `Route ${route.objectId}`}</p>
                        <p class="small faint">{facts(route)}</p>
                    </span>
                    <span class="tag" class:warn={expiry(route).startsWith("expir")}>{expiry(route)}</span>
                    {@render row?.(route, null)}
                    <button type="button" class="btn ghost" onclick={() => ondelete(route)}>
                        Delete
                    </button>
                </li>
            {/each}
        </ul>
    {/if}
</section>

<style>
    .sechead {
        display: flex;
        align-items: baseline;
        gap: 10px;
        margin-bottom: 8px;
    }

    .sechead h3 {
        margin: 0;
        font-size: 16.5px;
    }

    .rows {
        list-style: none;
        margin: 0;
        padding: 0;
    }

    .rows li {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 9px 0;
    }

    .rows li + li {
        border-top: 1px solid var(--line);
    }

    .rows p {
        margin: 0;
    }

    .grow {
        flex: 1;
        min-width: 0;
    }

    .name {
        font-family: var(--serif);
        font-size: 15.5px;
    }

    .name.faint {
        color: var(--ink-faint);
    }

    .tag {
        flex: none;
        font-size: 10.5px;
        letter-spacing: 0.04em;
        text-transform: uppercase;
        border: 1px solid var(--line-strong);
        border-radius: 999px;
        padding: 1px 8px;
        color: var(--ink-soft);
        white-space: nowrap;
    }

    .tag.warn {
        color: var(--coral);
        border-color: var(--coral);
    }

    .tripbox {
        border: 1px solid var(--line-strong);
        border-radius: 10px;
        margin: 10px 0;
        background: color-mix(in srgb, var(--parchment-3) 28%, transparent);
    }

    .tripbar {
        display: flex;
        align-items: baseline;
        gap: 10px;
        padding: 9px 12px;
        border-bottom: 1px solid var(--line);
    }

    .tripbar p {
        margin: 0;
    }

    .inset {
        padding: 0 12px 4px 22px;
    }

    .pad {
        margin: 0;
        padding: 8px 12px;
    }
</style>
