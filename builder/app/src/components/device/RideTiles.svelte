<!--
  The rides gallery: read-only over the cable, previewable, pullable. Each tile is the preview
  button; the one inner control is the pull-to-library icon (⤓), disabled once the library holds
  a durable copy — the "in library" / "not backed up" tag on the thumbnail says which.

  The read-only line is a statement of the protocol, not an apology from the UI: `deleteObject` on
  a ride is reserved and answered `notFound` — the device is the only place a ride can be deleted,
  which is what makes an unsynced ride impossible to lose from here.
-->
<script lang="ts">
    import Tile from "./Tile.svelte";
    import TrackThumb from "./TrackThumb.svelte";
    import { rideDistance, rideDuration } from "../../lib/device/rides";
    import type { Thumb } from "../../lib/device/thumbs.svelte";
    import type { RideListEntry } from "../../lib/usb/objects";

    let {
        rides,
        heldHere = null,
        trackFor,
        busy = false,
        pulling = false,
        onopen,
        onpull = null,
    }: {
        rides: readonly RideListEntry[];
        /** Ride ids a durable copy of which exists in the library, or null where no library exists
         *  to ask (the page only passes one on tiers with `platform.rides`). */
        heldHere?: ReadonlySet<number> | null;
        /** The thumbnail track of a ride, or null while it is still on its way. */
        trackFor: (rideId: number) => Thumb | null;
        /** True while a preview download holds the cable. */
        busy?: boolean;
        /** True while a pull job runs — the pull icons wait. */
        pulling?: boolean;
        onopen: (ride: RideListEntry) => void;
        /** Pull one ride to the library. Null on tiers without one: no icon at all. */
        onpull?: ((ride: RideListEntry) => void) | null;
    } = $props();

    function nameOf(ride: RideListEntry): string {
        return ride.name || `Ride ${ride.objectId}`;
    }

    function when(startTime: number): string {
        if (!startTime) return "date not recorded";
        // UTC, like the ride filenames: start_time is UTC seconds, and a late-evening ride would
        // otherwise be filed on the wrong day west of Greenwich.
        return new Date(startTime * 1000).toLocaleDateString(undefined, {
            year: "numeric",
            month: "short",
            day: "numeric",
            timeZone: "UTC",
        });
    }

    function facts(ride: RideListEntry): string {
        return [
            when(ride.startTime),
            rideDistance(ride.distanceM),
            rideDuration(ride.movingTimeS),
            `${ride.climbM.toLocaleString()} m up`,
        ].join(" · ");
    }
</script>

<div class="tilegrid">
    {#each rides as ride (ride.objectId)}
        {@const held = heldHere?.has(ride.objectId) ?? false}
        <Tile label={`Preview “${nameOf(ride)}”`} disabled={busy} onopen={() => onopen(ride)}>
            {#snippet thumb()}
                {@const track = trackFor(ride.objectId)}
                <TrackThumb segments={track ? [{ track }] : []}>
                    {#snippet tag()}
                        {#if heldHere}
                            {#if held}
                                <span class="tag ok">in library</span>
                            {:else}
                                <span class="tag warn">not backed up</span>
                            {/if}
                        {/if}
                    {/snippet}
                </TrackThumb>
            {/snippet}
            <span class="grow">
                <p class="name">{nameOf(ride)}</p>
                <p class="small faint">{facts(ride)}</p>
            </span>
            {#if onpull}
                <button
                    type="button"
                    class="iconbtn pull"
                    title="Pull to library"
                    aria-label="Pull to library"
                    disabled={pulling || held}
                    onclick={(e) => {
                        e.stopPropagation();
                        onpull?.(ride);
                    }}
                >
                    ⤓
                </button>
            {/if}
        </Tile>
    {/each}
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

    .tag {
        font-size: 10px;
        letter-spacing: 0.05em;
        text-transform: uppercase;
        border: 1px solid var(--line-strong);
        border-radius: 999px;
        padding: 1px 8px;
        white-space: nowrap;
        background: color-mix(in srgb, var(--panel) 80%, transparent);
    }

    .tag.ok {
        color: var(--forest);
        border-color: var(--forest);
    }

    .tag.warn {
        color: var(--coral);
        border-color: var(--coral);
    }

    .pull {
        color: var(--forest);
        border-color: var(--forest);
        font-size: 16px;
    }

    .pull:disabled {
        opacity: 0.35;
        cursor: default;
    }
</style>
