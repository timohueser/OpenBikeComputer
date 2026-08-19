<!--
  The routes gallery: every top-level route as a tile — thumbnail, serif name, size, one ⋯ menu —
  and the drop zone as the grid's last, ghost tile (rendered by the page through `children`, so this
  component stays presentation-only).

  What a tile can say is exactly what a `LIST` entry carries (§3.3): the id, the revision, the
  payload's length and CRC, the kind, the flags and a display name. A route's distance, ascent and
  point count are in the OBCR payload, so they appear when the rider opens the preview and the
  object is downloaded — not here, where showing them would cost a download per tile. Nothing is
  rendered as a placeholder in their place; a figure this side cannot know is simply not a line.

  The tile itself is the preview button (`Tile.svelte`); the ⋯ menu holds everything else. Every
  cable operation arrives as a callback from the page, which owns the queueing. The component's own
  state is exactly one thing — which name is being edited inline.
-->
<script lang="ts">
    import Tile from "./Tile.svelte";
    import TrackThumb from "./TrackThumb.svelte";
    import PopMenu from "./PopMenu.svelte";
    import type { TripView } from "../../lib/device/dashboard.svelte";
    import type { Thumb } from "../../lib/device/thumbs.svelte";
    import { formatBytes } from "../../lib/format";
    import { menuPick } from "../../lib/ui/menu";
    import type { CatalogEntry } from "../../lib/usb/protocol";

    let {
        routes,
        trips,
        trackFor,
        busy = false,
        onopen,
        onrename,
        ondelete,
        onaddtotrip,
        children,
    }: {
        routes: readonly CatalogEntry[];
        /** For the "Add to …" menu items. */
        trips: readonly TripView[];
        /** The thumbnail track of a route, or null while it is still on its way. */
        trackFor: (routeId: bigint) => Thumb | null;
        /** True while a preview download holds the cable — tiles wait their turn. */
        busy?: boolean;
        onopen: (route: CatalogEntry) => void;
        onrename: (route: CatalogEntry, name: string) => void;
        ondelete: (route: CatalogEntry) => void;
        /** Add a top-level route to a trip; `null` asks for a new trip around it. */
        onaddtotrip: (route: CatalogEntry, tripId: bigint | null) => void;
        /** The grid's last tile — the page renders the GPX drop zone here. */
        children?: import("svelte").Snippet;
    } = $props();

    /** The one name being edited inline, keyed by the route's `ObjectId`. */
    let editing = $state<{ id: bigint; value: string } | null>(null);

    function commitEdit(route: CatalogEntry) {
        const edit = editing;
        editing = null;
        if (!edit || !edit.value.trim()) return;
        if (edit.value.trim() !== route.displayName) onrename(route, edit.value);
    }

    function onEditKey(event: KeyboardEvent, route: CatalogEntry) {
        if (event.key === "Enter") commitEdit(route);
        if (event.key === "Escape") editing = null;
    }

    function nameOf(route: CatalogEntry): string {
        return route.displayName || `Route ${route.objectId}`;
    }
</script>

<div class="tilegrid">
    {#each routes as route (route.objectId)}
        <Tile label={`Preview “${nameOf(route)}”`} disabled={busy} onopen={() => onopen(route)}>
            {#snippet thumb()}
                {@const track = trackFor(route.objectId)}
                <!-- Coral, matching the detailed preview's track — rides draw forest, so the two
                     galleries tell apart at a glance. -->
                <TrackThumb segments={track ? [{ track, color: "var(--coral)" }] : []} />
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
                <p class="small faint">{formatBytes(Number(route.payloadLength))}</p>
            </span>
            <PopMenu label="Route actions">
                <button
                    type="button"
                    onclick={(e) => menuPick(e, () => (editing = { id: route.objectId, value: route.displayName }))}
                >
                    Rename…
                </button>
                {#each trips as trip (trip.objectId)}
                    <button type="button" onclick={(e) => menuPick(e, () => onaddtotrip(route, trip.objectId))}>
                        Add to “{trip.displayName || `Trip ${trip.objectId}`}”
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

</style>
