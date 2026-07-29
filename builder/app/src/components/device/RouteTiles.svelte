<!--
  The routes gallery: every top-level route as a tile — thumbnail, expiry tag, serif name, facts,
  one ⋯ menu — and the drop zone as the grid's last, ghost tile (rendered by the page through
  `children`, so this component stays presentation-only).

  The tile itself is the preview button (`Tile.svelte`); the ⋯ menu holds everything else,
  including the six-level "Keep on device" choice. Every cable operation arrives as a callback
  from the page, which owns the queueing. The component's own state is exactly one thing — which
  name is being edited inline.
-->
<script lang="ts">
    import Tile from "./Tile.svelte";
    import TrackThumb from "./TrackThumb.svelte";
    import PopMenu from "./PopMenu.svelte";
    import type { TripView } from "../../lib/device/dashboard.svelte";
    import { RETENTION_LEVELS, expiryPhrase, expiryWarns, retentionLabel } from "../../lib/device/retention";
    import type { Thumb } from "../../lib/device/thumbs.svelte";
    import { menuPick } from "../../lib/ui/menu";
    import type { RouteListEntry } from "../../lib/usb/objects";

    let {
        routes,
        trips,
        trackFor,
        busy = false,
        onopen,
        onrename,
        ondelete,
        onaddtotrip,
        onsetretention,
        children,
    }: {
        routes: readonly RouteListEntry[];
        /** For the "Add to …" menu items. */
        trips: readonly TripView[];
        /** The thumbnail track of a route, or null while it is still on its way. */
        trackFor: (routeId: number) => Thumb | null;
        /** True while a preview download holds the cable — tiles wait their turn. */
        busy?: boolean;
        onopen: (route: RouteListEntry) => void;
        onrename: (route: RouteListEntry, name: string) => void;
        ondelete: (route: RouteListEntry) => void;
        /** Add a top-level route to a trip; `null` asks for a new trip around it. */
        onaddtotrip: (route: RouteListEntry, tripId: number | null) => void;
        /** Set the §4.4 cmd 6 retention level of a stored route. */
        onsetretention: (route: RouteListEntry, level: number) => void;
        /** The grid's last tile — the page renders the GPX drop zone here. */
        children?: import("svelte").Snippet;
    } = $props();

    /** The one name being edited inline. */
    let editing = $state<{ id: number; value: string } | null>(null);

    function commitEdit(route: RouteListEntry) {
        const edit = editing;
        editing = null;
        if (!edit || !edit.value.trim()) return;
        if (edit.value.trim() !== route.name) onrename(route, edit.value);
    }

    function onEditKey(event: KeyboardEvent, route: RouteListEntry) {
        if (event.key === "Enter") commitEdit(route);
        if (event.key === "Escape") editing = null;
    }

    function nameOf(route: RouteListEntry): string {
        return route.name || `Route ${route.objectId}`;
    }

    function facts(route: RouteListEntry): string {
        const parts = [`${(route.distanceM / 1000).toFixed(1)} km`, `${route.ascentM.toLocaleString()} m up`];
        if (route.waypointCount > 0)
            parts.push(`${route.waypointCount} waypoint${route.waypointCount === 1 ? "" : "s"}`);
        return parts.join(" · ");
    }
</script>

<div class="tilegrid">
    {#each routes as route (route.objectId)}
        <Tile label={`Preview “${nameOf(route)}”`} disabled={busy} onopen={() => onopen(route)}>
            {#snippet thumb()}
                {@const track = trackFor(route.objectId)}
                <TrackThumb segments={track ? [{ track }] : []}>
                    {#snippet tag()}
                        <span class="tag" class:warn={expiryWarns(route)}>{expiryPhrase(route)}</span>
                    {/snippet}
                </TrackThumb>
            {/snippet}
            <span class="grow">
                {#if editing && editing.id === route.objectId}
                    <!-- svelte-ignore a11y_autofocus -->
                    <input
                        class="rename"
                        type="text"
                        autofocus
                        bind:value={editing.value}
                        onblur={() => commitEdit(route)}
                        onkeydown={(e) => onEditKey(e, route)}
                    />
                {:else}
                    <p class="name">{nameOf(route)}</p>
                {/if}
                <p class="small faint">{facts(route)}</p>
            </span>
            <PopMenu label="Route actions">
                <button
                    type="button"
                    onclick={(e) => menuPick(e, () => (editing = { id: route.objectId, value: route.name }))}
                >
                    Rename…
                </button>
                <details>
                    <summary>Keep on device…</summary>
                    {#each RETENTION_LEVELS as level (level)}
                        <button type="button" onclick={(e) => menuPick(e, () => onsetretention(route, level))}>
                            <span class="check" class:on={route.retention === level}>✓</span>
                            {retentionLabel(level)}
                        </button>
                    {/each}
                </details>
                {#each trips as trip (trip.objectId)}
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
            </PopMenu>
        </Tile>
    {/each}
    {@render children?.()}
</div>

<style>
    .grow {
        flex: 1;
        min-width: 0;
    }

    .grow p {
        margin: 0;
    }

    .name {
        font-family: var(--serif);
        font-size: 15.5px;
        font-weight: 600;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .rename {
        font-family: var(--serif);
        font-size: 15.5px;
        color: var(--ink);
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 7px;
        padding: 2px 8px;
        width: 100%;
    }

    .tag {
        font-size: 10px;
        letter-spacing: 0.05em;
        text-transform: uppercase;
        border: 1px solid var(--line-strong);
        border-radius: 999px;
        padding: 1px 8px;
        color: var(--ink-soft);
        white-space: nowrap;
        background: color-mix(in srgb, var(--panel) 80%, transparent);
    }

    .tag.warn {
        color: var(--coral);
        border-color: var(--coral);
    }

    .check {
        display: inline-block;
        width: 12px;
        margin-left: -14px;
        visibility: hidden;
    }

    .check.on {
        visibility: visible;
        color: var(--forest);
    }
</style>
