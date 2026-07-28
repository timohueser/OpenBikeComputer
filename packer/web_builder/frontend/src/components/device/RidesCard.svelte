<!--
  Rides, as the card holds them: read-only over the cable, previewable, pullable.

  The read-only line is a statement of the protocol, not an apology from the UI: `deleteObject` on a
  ride is reserved and answered `notFound` — the device is the only place a ride can be deleted,
  which is what makes an unsynced ride impossible to lose from here.
-->
<script lang="ts">
    import { dashboard } from "../../lib/device/dashboard.svelte";
    import { rideDistance, rideDuration } from "../../lib/device/rides";
    import type { RideListEntry } from "../../lib/usb/objects";

    let {
        heldHere = null,
        row,
        actions,
    }: {
        /** Ride ids a durable copy of which exists in the library, or null where no library exists
         *  to ask (the page only passes one on tiers with `platform.rides`). */
        heldHere?: ReadonlySet<number> | null;
        /** Per-ride actions (preview, pull) — wired by the page. */
        row?: import("svelte").Snippet<[RideListEntry]>;
        /** The card-level actions beside the heading (pull all). */
        actions?: import("svelte").Snippet;
    } = $props();

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

<section class="card">
    <div class="sechead">
        <h3>Rides</h3>
        <span class="small faint">{dashboard.rides.length} on the device</span>
        <span class="spacer">{@render actions?.()}</span>
    </div>

    {#if dashboard.ridesTruncated}
        <p class="small faint">
            The device listed its newest rides only; older ones are still on the card.
        </p>
    {/if}

    {#if dashboard.rides.length === 0}
        <p class="small muted">No rides on the device.</p>
    {:else}
        <ul class="rows">
            {#each dashboard.rides as ride (ride.objectId)}
                <li>
                    <span class="grow">
                        <p class="name">{ride.name || `Ride ${ride.objectId}`}</p>
                        <p class="small faint">{facts(ride)}</p>
                    </span>
                    {#if heldHere}
                        {#if heldHere.has(ride.objectId)}
                            <span class="tag ok">in library</span>
                        {:else}
                            <span class="tag warn">not backed up</span>
                        {/if}
                    {/if}
                    {@render row?.(ride)}
                </li>
            {/each}
        </ul>
    {/if}

    <p class="small faint disclosure">
        Rides are renamed and deleted on the device itself — over the cable they are read-only, so a
        ride can never be lost.
    </p>
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

    .spacer {
        margin-left: auto;
        display: flex;
        gap: 8px;
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

    .tag {
        flex: none;
        font-size: 10.5px;
        letter-spacing: 0.04em;
        text-transform: uppercase;
        border: 1px solid var(--line-strong);
        border-radius: 999px;
        padding: 1px 8px;
        white-space: nowrap;
    }

    .tag.ok {
        color: var(--forest);
        border-color: var(--forest);
    }

    .tag.warn {
        color: var(--coral);
        border-color: var(--coral);
    }

    .disclosure {
        margin: 10px 0 0;
    }

    p {
        margin: 0;
    }
</style>
