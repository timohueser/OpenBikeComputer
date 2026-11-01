<script lang="ts">
    import { onMount } from "svelte";
    import { counts, STATUSES, type SourceLayers } from "../../lib/data/holding";
    import { CliQuery } from "../../lib/data/query.svelte";
    import Failure from "./Failure.svelte";

    // Each name of `used_by` with a chip for each source that it reads, in the colour of its
    // status, from `obc data sources --layer`.
    const query = new CliQuery<SourceLayers>(["sources", "--layer"]);
    onMount(() => void query.refresh());
</script>

{#if query.error}<Failure error={query.error} />{/if}
<p class="soft">Whether a bake still downloads depends on its region: By region tells.</p>
<div class="layers">
    {#each query.output?.layers ?? [] as { layer, sources } (layer)}
        <section class="card" aria-labelledby="layer-{layer}">
            <div class="title">
                <h2 id="layer-{layer}">{layer}</h2>
                <span class="soft">{counts(sources.map((at) => at.held)).filter(([, n]) => n).map(([status, n]) => `${n} ${STATUSES[status].toLowerCase()}`).join(" · ")}</span>
            </div>
            <ul class="chips">
                {#each sources as { source, held } (source)}
                    <li>
                        <a class="pill" data-status={held.status} href="#/sources/{encodeURIComponent(source)}">{source}<span class="visually-hidden">, {STATUSES[held.status]}</span></a>
                    </li>
                {/each}
            </ul>
        </section>
    {/each}
</div>
<ul class="chips" aria-label="Key">
    {#each Object.entries(STATUSES) as [status, label] (status)}<li><span class="pill" data-status={status}>{label}</span></li>{/each}
</ul>

<style>
    .layers {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(min(100%, 420px), 1fr));
        gap: 16px;
    }
    .chips {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    a.pill {
        font-family: var(--mono);
        font-weight: 500;
    }
    a.pill:hover {
        border-color: var(--tone);
    }
</style>
