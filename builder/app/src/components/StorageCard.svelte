<script lang="ts">
    // What the app has put on this disk, and the button that takes it back.
    //
    // This is not a settings screen: it is here because the numbers are large
    // enough to be someone's problem. A country extract is hundreds of megabytes
    // and the shared land-polygon dataset is over two gigabytes, and neither was
    // ever chosen by the user — they are the price of a build. So each row states
    // a real path, a real size, and what deleting it costs on the next build.
    //
    // Mounted only where `platform.storage` exists (see `DiskStorage`), which is
    // a member check, not a host name.

    import { onMount } from "svelte";
    import { formatBytes } from "../lib/format";
    import { confirmAction } from "../lib/ui/confirm.svelte";
    import type { DiskStorage, StoragePlace } from "../lib/platform/types";

    let { storage }: { storage: DiskStorage } = $props();

    let places = $state<StoragePlace[]>([]);
    let error = $state<string | null>(null);
    let busy = $state<string | null>(null);

    async function load() {
        try {
            places = await storage.places();
            error = null;
        } catch (e) {
            error = e instanceof Error ? e.message : String(e);
        }
    }

    onMount(load);

    async function clear(place: StoragePlace) {
        // Deleting the land dataset means a ~950 MB re-download; deleting an
        // extract means one region. The confirm carries the size so the answer
        // isn't guesswork.
        const ok = await confirmAction({
            title: `Delete ${formatBytes(place.bytes)} from ${place.path}?`,
            body: place.note,
            confirmLabel: "Delete",
            destructive: true,
        });
        if (!ok) return;
        busy = place.id;
        try {
            await storage.clear(place.id);
            await load();
        } catch (e) {
            error = e instanceof Error ? e.message : String(e);
        } finally {
            busy = null;
        }
    }

    const total = $derived(places.reduce((sum, p) => sum + p.bytes, 0));
</script>

<section class="card">
    <div class="step-head">
        <h3>On this machine</h3>
        <span class="small faint">{formatBytes(total)}</span>
    </div>

    {#if error}
        <p class="small" style:color="var(--coral)">{error}</p>
    {/if}

    <ul class="places">
        {#each places as place (place.id)}
            <li>
                <div class="line">
                    <span class="label">{place.label}</span>
                    <span class="small faint size">{formatBytes(place.bytes)}</span>
                    {#if place.clearable}
                        <button
                            type="button"
                            class="btn small-btn"
                            disabled={busy !== null || place.bytes === 0}
                            onclick={() => clear(place)}
                        >
                            {busy === place.id ? "Clearing…" : "Clear"}
                        </button>
                    {/if}
                </div>
                <p class="small faint mono path">{place.path}</p>
                <p class="small muted note">{place.note}</p>
            </li>
        {/each}
    </ul>
</section>

<style>
    .step-head {
        display: flex;
        align-items: center;
        gap: 9px;
        margin-bottom: 10px;
    }

    .step-head h3 {
        font-size: 16.5px;
    }

    .step-head .small {
        margin-left: auto;
    }

    .places {
        list-style: none;
        margin: 0;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 12px;
    }

    .line {
        display: flex;
        align-items: center;
        gap: 8px;
    }

    .label {
        font-size: 14px;
    }

    .size {
        margin-left: auto;
    }

    .small-btn {
        padding: 2px 10px;
        font-size: 12.5px;
    }

    .path,
    .note {
        margin: 2px 0 0;
        word-break: break-all;
    }
</style>
