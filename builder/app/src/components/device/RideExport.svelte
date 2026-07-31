<!--
  The rides on the device, and a GPX out of any one of them (C5, #904).

  The only surface here that *reads* from the device instead of writing to it, and the only one
  whose central design question is what it must not do. `synced` on the device means "a durable copy
  exists off the device" — it guards deletes and anchors the auto-expiry countdown (#638) — and a
  browser download is not durable: the rider can cancel at the save dialog. So this panel never
  acks, and the panel is handed a `RideSource` rather than a client so that it cannot (see
  `lib/device/rides.ts`).

  That has a consequence the rider has to be told, in one line, right here: an export is not a
  backup. Someone who believes it was, and then lets the device auto-delete, has lost a ride. The
  sentence sits under the list rather than in a tooltip for exactly that reason.

  The catalog is read once, on mount, and there is deliberately no subscription to the store's
  change signal: a ride is only created by finishing one, and nobody finishes a ride with a cable
  plugged in. A reconnect remounts this panel anyway, which is the one case where the list could
  really have gone stale.
-->
<script lang="ts">
    import { onMount } from "svelte";
    import Gated from "../Gated.svelte";
    import TransferBar from "./TransferBar.svelte";
    import { saveBytes } from "../../lib/download";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import {
        exportRide,
        rideDistance,
        rideDuration,
        rideKey,
        scopeKey,
        type RideListEntry,
        type RideScope,
        type RideSource,
    } from "../../lib/device/rides";
    import { formatBytes } from "../../lib/format";
    import { initConvert } from "../../lib/convert/bridge";

    let { rides, scope }: { rides: RideSource; scope: RideScope } = $props();

    const job = new DeviceJob("ride");

    let entries = $state<RideListEntry[]>([]);
    let truncated = $state(false);
    let loading = $state(true);
    let listError = $state<string | null>(null);
    /**
     * Rides exported in *this visit*, keyed by `(serial, epoch, id)`.
     *
     * Not a record of anything — it is never persisted, and it is thrown away the moment the id era
     * changes, because a store-epoch bump recycles ids and a tick against a recycled id would be a
     * claim about a ride nobody has seen. It exists so the row says "exported" instead of nothing,
     * which is a different word from "synced" on purpose.
     */
    let exported = $state(new Set<string>());
    let lastScope = "";

    // The wasm exporter is ~95 KB and loads on demand. Starting it while the list is being read
    // turns the conversion at the end of the pull into a plain function call.
    onMount(() => {
        void initConvert();
        void refresh();
    });

    $effect(() => {
        const key = scopeKey(scope);
        if (key !== lastScope) {
            lastScope = key;
            exported = new Set();
        }
    });

    async function refresh() {
        loading = entries.length === 0;
        listError = null;
        try {
            const catalog = await rides.listRides();
            entries = [...catalog.entries].sort((a, b) => b.startTime - a.startTime || b.objectId - a.objectId);
            truncated = catalog.truncated;
        } catch (cause) {
            listError = cause instanceof Error ? cause.message : String(cause);
        } finally {
            loading = false;
        }
    }

    async function save(entry: RideListEntry) {
        const result = await job.run(
            (ctx) => exportRide(rides, entry, ctx),
            (value) => `Saved ${value.filename} — ${value.points.toLocaleString()} points.`,
        );
        if (!result) return;
        saveBytes(new TextEncoder().encode(result.gpx), result.filename, "application/gpx+xml");
        exported = new Set(exported).add(rideKey(scope, entry.objectId));
    }

    /** The one line under a ride's name: when, how far, how long, how big. */
    function facts(entry: RideListEntry): string {
        const when = entry.startTime
            ? new Date(entry.startTime * 1000).toLocaleDateString(undefined, {
                  year: "numeric",
                  month: "short",
                  day: "numeric",
              })
            : "date not recorded";
        return [when, rideDistance(entry.distanceM), rideDuration(entry.movingTimeS), formatBytes(entry.byteLen)].join(
            " · ",
        );
    }
</script>

<section class="block">
    <h4>Rides</h4>

    {#if listError}
        <p class="note error small" role="alert">{listError}</p>
        <button type="button" class="btn ghost" onclick={() => void refresh()}>Try again</button>
    {:else if loading}
        <p class="small muted">Reading the device's rides…</p>
    {:else if entries.length === 0}
        <p class="small muted">No rides recorded on this device yet.</p>
    {:else}
        <ul class="rides">
            {#each entries as entry (entry.objectId)}
                <li>
                    <div class="what">
                        <p class="name">
                            {entry.name || `Ride ${entry.objectId}`}
                            {#if exported.has(rideKey(scope, entry.objectId))}
                                <span class="tag">exported</span>
                            {/if}
                        </p>
                        <p class="small faint">{facts(entry)}</p>
                    </div>
                    <button type="button" class="btn" disabled={job.running} onclick={() => void save(entry)}>
                        Export GPX
                    </button>
                </li>
            {/each}
        </ul>

        {#if truncated}
            <p class="small faint">
                The device listed its newest rides only; older ones are still on the card.
            </p>
        {/if}

        <p class="small muted">
            An export is not a backup — the device is not told, the ride stays unsynced there, and
            the file you save is the only copy.
        </p>
        <Gated need="rideLibrary" />
    {/if}

    <TransferBar {job} />
</section>

<style>
    h4 {
        margin: 0 0 6px;
        font-size: 14px;
        font-family: var(--sans);
        letter-spacing: 0.02em;
        text-transform: uppercase;
        color: var(--ink-faint);
    }

    .rides {
        list-style: none;
        margin: 0 0 10px;
        padding: 0;
    }

    .rides li {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 8px 0;
    }

    .rides li + li {
        border-top: 1px solid var(--line);
    }

    .what {
        margin-right: auto;
        min-width: 0;
    }

    .what p {
        margin: 0;
    }

    .name {
        font-family: var(--serif);
        font-size: 16px;
    }

    .tag {
        margin-left: 6px;
        font-family: var(--sans);
        font-size: 11px;
        letter-spacing: 0.04em;
        text-transform: uppercase;
        color: var(--ink-faint);
    }

    .note {
        margin: 8px 0 0;
    }

    .error {
        color: var(--coral);
    }
</style>
