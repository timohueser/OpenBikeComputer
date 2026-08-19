<!--
  The rides gallery: read-only over the cable, previewable, pullable. Each tile is the preview
  button; the one inner control is the pull-to-library icon (⤓), disabled once the library holds
  a durable copy — the "in library" / "not backed up" tag on the thumbnail says which.

  The read-only line is a statement of the protocol, not an apology from the UI: this page holds no
  path that writes a ride, so the device is the only place a ride can be renamed or deleted — which
  is what makes an uncopied ride impossible to lose from here.

  A ride the device is **still recording** is listed and not offered. §3.5 refuses a `GET` of an
  entry carrying `RECORDING` — its payload length and CRC are zero until the commit that ends the
  ride — so the tile is drawn, marked, and inert, rather than being a click into a guaranteed error.

  What the rest of a tile can say is what a `LIST` entry carries (§3.3): the name and the payload's
  size. A ride's start time, distance, duration and climb live in the ride object; they appear in
  the preview, which downloads it. Where one of those figures used to sit, nothing sits now.
-->
<script lang="ts">
    import Tile from "./Tile.svelte";
    import TrackThumb from "./TrackThumb.svelte";
    import { formatBytes } from "../../lib/format";
    import type { Thumb } from "../../lib/device/thumbs.svelte";
    import { EntryFlags, type CatalogEntry } from "../../lib/usb/protocol";

    let {
        rides,
        heldHere = null,
        trackFor,
        busy = false,
        pulling = false,
        onopen,
        onpull = null,
    }: {
        rides: readonly CatalogEntry[];
        /** Ride ids a durable copy of which exists in the library, or null where no library exists
         *  to ask (the page only passes one on tiers with `platform.rides`). */
        heldHere?: ReadonlySet<bigint> | null;
        /** The thumbnail track of a ride, or null while it is still on its way. */
        trackFor: (rideId: bigint) => Thumb | null;
        /** True while a preview download holds the cable. */
        busy?: boolean;
        /** True while a pull job runs — the pull icons wait. */
        pulling?: boolean;
        onopen: (ride: CatalogEntry) => void;
        /** Pull one ride to the library. Null on tiers without one: no icon at all. */
        onpull?: ((ride: CatalogEntry) => void) | null;
    } = $props();

    function nameOf(ride: CatalogEntry): string {
        return ride.displayName || `Ride ${ride.objectId}`;
    }

    const isRecording = (ride: CatalogEntry): boolean => (ride.flags & EntryFlags.Recording) !== 0;
</script>

<div class="tilegrid">
    {#each rides as ride (ride.objectId)}
        {@const recording = isRecording(ride)}
        {@const held = heldHere?.has(ride.objectId) ?? false}
        <Tile
            label={recording ? `“${nameOf(ride)}” is still being recorded` : `Preview “${nameOf(ride)}”`}
            disabled={busy || recording}
            onopen={() => onopen(ride)}
        >
            {#snippet thumb()}
                {@const track = trackFor(ride.objectId)}
                <!-- Forest — the ride color, against the routes' coral (and the library rows'
                     matching forest previews). -->
                <TrackThumb segments={track ? [{ track, color: "var(--forest)" }] : []}>
                    {#snippet tag()}
                        {#if recording}
                            <span class="tag warn">recording</span>
                        {:else if heldHere}
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
                <!-- A recording ride's entry declares a zero length until the commit that ends it,
                     so its size is not a fact yet and is not shown as one. -->
                <p class="small faint">
                    {recording ? "still on the device" : formatBytes(Number(ride.payloadLength))}
                </p>
            </span>
            {#if onpull}
                <button
                    type="button"
                    class="iconbtn pull"
                    title="Pull to library"
                    aria-label="Pull to library"
                    disabled={pulling || held || recording}
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
