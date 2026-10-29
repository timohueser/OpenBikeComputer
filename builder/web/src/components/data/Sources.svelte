<script lang="ts">
    import { onMount } from "svelte";
    import type { Areas } from "../../lib/data/areas";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { sourceAt, viewHash, type SourceView } from "../../lib/data/screens";
    import type { Status } from "../../lib/data/shapes";
    import type { SourceRow } from "../../lib/data/sources";
    import Failure from "./Failure.svelte";
    import Prepare from "./Prepare.svelte";
    import SourceCard from "./SourceCard.svelte";
    import SourceLayers from "./SourceLayers.svelte";
    import SourceList from "./SourceList.svelte";
    import SourceRegion from "./SourceRegion.svelte";

    // Every source that a bake or a prepare reads, and how this machine holds it: by source, by
    // layer or for a region. A source opens its card.
    let { hash, status }: { hash: string; status: CliQuery<Status> } = $props();

    const sources = new CliQuery<{ sources: SourceRow[] }>(["sources"]);
    const areas = new CliQuery<Areas>(["areas"]);
    onMount(() => {
        void sources.refresh();
        void areas.refresh();
    });
    const route = $derived(sourceAt(hash));
    const rows = $derived(sources.output?.sources ?? []);
    const row = $derived(rows.find((at) => at.id === route.id));
    const VIEWS: Record<SourceView, string> = { source: "All sources", layer: "By layer", region: "By region" };
</script>

{#if sources.error}<Failure error={sources.error} />{/if}
{#if areas.error}<Failure error={areas.error} />{/if}

{#if route.id && route.prepare}
    {#key route.id}<Prepare id={route.id} {rows} area={route.area} areas={areas.output} />{/key}
{:else if route.id}
    {#if row}
        {#key row.id}<SourceCard {row} {rows} {status} areas={areas.output} />{/key}
    {:else if sources.output}
        <a href="#/sources">All sources</a>
        <p class="missing">data/sources.toml has no source {route.id}.</p>
    {/if}
{:else}
    <div class="title">
        <h1>Sources</h1>
        <div class="segmented" role="group" aria-label="View">
            {#each Object.entries(VIEWS) as [by, label] (by)}
                <button type="button" aria-pressed={route.by === by} onclick={() => (location.hash = viewHash(by as SourceView))}>{label}</button>
            {/each}
        </div>
    </div>
    {#if route.by === "layer"}
        <SourceLayers {rows} />
    {:else if route.by === "region"}
        {#key route.area?.value}<SourceRegion region={route.area?.flag === "--region" ? route.area.value : null} {rows} areas={areas.output} />{/key}
    {:else}
        <SourceList {rows} status={status.output} />
    {/if}
{/if}
