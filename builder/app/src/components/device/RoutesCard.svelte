<!--
  Routes & trips, as the card holds them: trips render as groups of their stage routes — the same
  grouping the device's own UI shows — and every route outside a trip is a top-level row.

  Presentation only: every cable operation arrives as a callback from the page, which owns the
  queueing. The card's own state is exactly one thing — which name is being edited inline.

  Row shape (the wireframe's rule): one visible action, one ⋯ menu. The menu is a `<details>`
  element — no popover machinery exists in this codebase, and none is needed for four items.
  Facts come straight off the list entries; the one editorial addition is the expiry column,
  which the wire has always carried (`expiresAt`/`retention`) and no UI had ever shown.
-->
<script lang="ts">
    import { dashboard, type TripView } from "../../lib/device/dashboard.svelte";
    import type { RouteListEntry } from "../../lib/usb/objects";
    import { formatBytes } from "../../lib/format";

    let {
        onpreview = null,
        onrename,
        ondelete,
        onaddtotrip,
        onrenametrip,
        ondeletetrip,
        onremovestage,
        onmovestage,
    }: {
        /** Preview a route (downloads it). Null until the page can show one. */
        onpreview?: ((route: RouteListEntry) => void) | null;
        onrename: (route: RouteListEntry, name: string) => void;
        ondelete: (route: RouteListEntry) => void;
        /** Add a top-level route to a trip; `null` asks for a new trip around it. */
        onaddtotrip: (route: RouteListEntry, tripId: number | null) => void;
        onrenametrip: (trip: TripView, name: string) => void;
        ondeletetrip: (trip: TripView) => void;
        onremovestage: (trip: TripView, index: number) => void;
        onmovestage: (trip: TripView, index: number, delta: number) => void;
    } = $props();

    /** The one name being edited inline, route or trip. */
    let editing = $state<{ kind: "route" | "trip"; id: number; value: string } | null>(null);

    function startEdit(kind: "route" | "trip", id: number, value: string) {
        editing = { kind, id, value };
    }

    function commitEdit(route: RouteListEntry | null, trip: TripView | null) {
        const edit = editing;
        editing = null;
        if (!edit || !edit.value.trim()) return;
        if (route && edit.value.trim() !== route.name) onrename(route, edit.value);
        if (trip && edit.value.trim() !== trip.name) onrenametrip(trip, edit.value);
    }

    function onEditKey(event: KeyboardEvent, route: RouteListEntry | null, trip: TripView | null) {
        if (event.key === "Enter") commitEdit(route, trip);
        if (event.key === "Escape") editing = null;
    }

    /** Close the enclosing ⋯ menu, then run the action — a `<details>` does not close itself. */
    function menuPick(event: MouseEvent, action: () => void) {
        (event.currentTarget as HTMLElement).closest("details")?.removeAttribute("open");
        action();
    }

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

{#snippet nameOrEdit(kind: "route" | "trip", id: number, name: string, route: RouteListEntry | null, trip: TripView | null)}
    {#if editing && editing.kind === kind && editing.id === id}
        <!-- svelte-ignore a11y_autofocus -->
        <input
            class="rename"
            type="text"
            autofocus
            bind:value={editing.value}
            onblur={() => commitEdit(route, trip)}
            onkeydown={(e) => onEditKey(e, route, trip)}
        />
    {:else}
        <p class="name">{name}</p>
    {/if}
{/snippet}

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
                {@render nameOrEdit("trip", trip.objectId, trip.name || `Trip ${trip.objectId}`, null, trip)}
                <span class="tag">trip</span>
                <p class="small faint grow">{tripFacts(trip)}</p>
                <details class="menu">
                    <summary class="btn ghost" aria-label="Trip actions">⋯</summary>
                    <div class="pop" role="menu">
                        <button
                            type="button"
                            onclick={(e) => menuPick(e, () => startEdit("trip", trip.objectId, trip.name))}
                        >
                            Rename…
                        </button>
                        <button type="button" class="danger" onclick={(e) => menuPick(e, () => ondeletetrip(trip))}>
                            Delete trip…
                        </button>
                    </div>
                </details>
            </div>
            {#if trip.detail === null}
                <p class="small faint pad">The trip's stage list could not be read.</p>
            {:else}
                <ul class="rows inset">
                    {#each dashboard.stagesOf(trip) as stage, index (`${stage.id}-${index}`)}
                        {@const stages = trip.detail.stages.length}
                        <li>
                            <span class="order">
                                <button
                                    type="button"
                                    class="nudge"
                                    disabled={index === 0}
                                    aria-label="Move up"
                                    onclick={() => onmovestage(trip, index, -1)}>↑</button>
                                <button
                                    type="button"
                                    class="nudge"
                                    disabled={index === stages - 1}
                                    aria-label="Move down"
                                    onclick={() => onmovestage(trip, index, 1)}>↓</button>
                            </span>
                            {#if stage.route}
                                {@const route = stage.route}
                                <span class="grow">
                                    <p class="name">{route.name || `Route ${stage.id}`}</p>
                                    <p class="small faint">{facts(route)}</p>
                                </span>
                                {#if onpreview}
                                    <button type="button" class="btn" onclick={() => onpreview?.(route)}>
                                        Preview
                                    </button>
                                {/if}
                            {:else}
                                <span class="grow">
                                    <p class="name faint">Route {stage.id}</p>
                                    <p class="small faint">no longer on the device</p>
                                </span>
                            {/if}
                            <button
                                type="button"
                                class="btn ghost"
                                title="Remove from trip (the route stays on the device)"
                                aria-label="Remove from trip"
                                onclick={() => onremovestage(trip, index)}>×</button>
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
                        {@render nameOrEdit("route", route.objectId, route.name || `Route ${route.objectId}`, route, null)}
                        <p class="small faint">{facts(route)}</p>
                    </span>
                    <span class="tag" class:warn={expiry(route).startsWith("expir")}>{expiry(route)}</span>
                    {#if onpreview}
                        <button type="button" class="btn" onclick={() => onpreview?.(route)}>Preview</button>
                    {/if}
                    <details class="menu">
                        <summary class="btn ghost" aria-label="Route actions">⋯</summary>
                        <div class="pop" role="menu">
                            <button
                                type="button"
                                onclick={(e) => menuPick(e, () => startEdit("route", route.objectId, route.name))}
                            >
                                Rename…
                            </button>
                            {#each dashboard.trips as trip (trip.objectId)}
                                <button type="button" onclick={(e) => menuPick(e, () => onaddtotrip(route, trip.objectId))}>
                                    Add to “{trip.name || `Trip ${trip.objectId}`}”
                                </button>
                            {/each}
                            <button type="button" onclick={(e) => menuPick(e, () => onaddtotrip(route, null))}>
                                New trip from this route
                            </button>
                            <button type="button" class="danger" onclick={(e) => menuPick(e, () => ondelete(route))}>
                                Delete…
                            </button>
                        </div>
                    </details>
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

    .rows p,
    .tripbar p {
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

    .rename {
        font-family: var(--serif);
        font-size: 15.5px;
        color: var(--ink);
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 7px;
        padding: 2px 8px;
        min-width: 200px;
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

    .inset {
        padding: 0 12px 4px 16px;
    }

    .pad {
        margin: 0;
        padding: 8px 12px;
    }

    .order {
        display: flex;
        flex-direction: column;
        gap: 1px;
        flex: none;
    }

    .nudge {
        border: 0;
        background: none;
        padding: 0 3px;
        font-size: 11px;
        line-height: 1.2;
        color: var(--ink-faint);
        cursor: pointer;
    }

    .nudge:hover:not(:disabled) {
        color: var(--ink);
    }

    .nudge:disabled {
        opacity: 0.3;
        cursor: default;
    }

    /* --- the ⋯ menu ------------------------------------------------------- */

    .menu {
        position: relative;
        flex: none;
    }

    .menu summary {
        list-style: none;
        cursor: pointer;
        user-select: none;
    }

    .menu summary::-webkit-details-marker {
        display: none;
    }

    .pop {
        position: absolute;
        right: 0;
        top: calc(100% + 4px);
        z-index: 30;
        min-width: 180px;
        display: flex;
        flex-direction: column;
        padding: 4px;
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 9px;
        box-shadow: 0 8px 22px rgba(36, 51, 28, 0.18);
    }

    .pop button {
        border: 0;
        background: none;
        text-align: left;
        font: inherit;
        font-size: 13px;
        color: var(--ink);
        padding: 6px 10px;
        border-radius: 6px;
        cursor: pointer;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .pop button:hover {
        background: var(--parchment-2);
    }

    .pop .danger {
        color: var(--coral);
    }
</style>
