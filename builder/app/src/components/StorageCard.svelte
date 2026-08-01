<script lang="ts">
    import { onMount } from "svelte";
    import { formatBytes } from "../lib/format";
    import type { DiskStorage, StoragePlace } from "../lib/platform/types";

    let { storage }: { storage: DiskStorage } = $props();

    let places = $state<StoragePlace[]>([]);
    let error = $state<string | null>(null);

    async function load() {
        try {
            places = await storage.places();
            error = null;
        } catch (e) {
            error = e instanceof Error ? e.message : String(e);
        }
    }

    onMount(load);

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

    .path,
    .note {
        margin: 2px 0 0;
        word-break: break-all;
    }
</style>
