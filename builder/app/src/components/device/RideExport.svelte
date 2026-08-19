<!--
  The rides on the device, and a GPX out of any one of them (C5, #904).

  The only surface here that *reads* from the device instead of writing to it. It is handed a
  `RideSource` rather than a client, so "this panel cannot change anything on the device" is a
  property of the object it holds rather than of what it remembers not to call
  (`lib/device/rides.ts`).

  Nothing on this path tells the device anything, and that is now true of every USB peer: §5.2.2
  retires the v1 possession acknowledgement, because an ack changes no object and so has no store
  meaning. The rider still has to be told the consequence, in one line, right here — an export is
  not a backup. The file that lands in a Downloads folder is the only copy, and the device does not
  know it exists.

  The catalog is read once, on mount, and there is deliberately no subscription to the store's
  commit sequence: a ride is only created by finishing one, and nobody finishes a ride with a cable
  plugged in. A reconnect remounts this panel anyway, which is the one case where the list could
  really have gone stale.

  A `LIST` entry carries the name and the payload's size and nothing else about a ride (§3.3), so
  that is the line under each name. The start time, distance and duration are in the ride object,
  which only an export downloads.
-->
<script lang="ts">
    import { onMount } from "svelte";
    import Gated from "../Gated.svelte";
    import TransferBar from "./TransferBar.svelte";
    import { saveBytes } from "../../lib/download";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import {
        exportRide,
        recordedRides,
        rideKey,
        scopeKey,
        type CatalogEntry,
        type RideScope,
        type RideSource,
    } from "../../lib/device/rides";
    import { formatBytes } from "../../lib/format";
    import { initConvert } from "../../lib/convert/bridge";

    let { rides, scope }: { rides: RideSource; scope: RideScope } = $props();

    const job = new DeviceJob("ride");

    let entries = $state<CatalogEntry[]>([]);
    let loading = $state(true);
    let listError = $state<string | null>(null);
    /**
     * Rides exported in *this visit*, keyed by `(serial, era, id)`.
     *
     * Not a record of anything — it is never persisted, and it is thrown away the moment the id era
     * changes, because a re-initialized card starts its ids again and a tick against a restarted id
     * would be a claim about a ride nobody has seen. It exists so the row says "exported" instead
     * of nothing.
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
            // Newest first, by `ObjectId`. A `LIST` entry carries no start time, and the id comes
            // from a monotonic allocation cursor never reused within one card
            // (`FLAT_Store_Format.md` §3) — so on one card, id order *is* recording order.
            //
            // `recordedRides` drops what is still being recorded: §3.5 refuses a `GET` of an entry
            // carrying `RECORDING`, so such a row could only ever be an error to click.
            entries = recordedRides(await rides.listRides()).sort((a, b) =>
                a.objectId < b.objectId ? 1 : a.objectId > b.objectId ? -1 : 0,
            );
        } catch (cause) {
            listError = cause instanceof Error ? cause.message : String(cause);
        } finally {
            loading = false;
        }
    }

    async function save(entry: CatalogEntry) {
        const result = await job.run(
            (ctx) => exportRide(rides, entry, ctx),
            (value) => `Saved ${value.filename} — ${value.points.toLocaleString()} points.`,
        );
        if (!result) return;
        saveBytes(new TextEncoder().encode(result.gpx), result.filename, "application/gpx+xml");
        exported = new Set(exported).add(rideKey(scope, entry.objectId));
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
                            {entry.displayName || `Ride ${entry.objectId}`}
                            {#if exported.has(rideKey(scope, entry.objectId))}
                                <span class="tag">exported</span>
                            {/if}
                        </p>
                        <p class="small faint">{formatBytes(Number(entry.payloadLength))}</p>
                    </div>
                    <button type="button" class="btn" disabled={job.running} onclick={() => void save(entry)}>
                        Export GPX
                    </button>
                </li>
            {/each}
        </ul>

        <p class="small muted">
            An export is not a backup — the device is not told anything, and the file you save is the
            only copy.
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
