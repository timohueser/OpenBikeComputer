<script lang="ts">
    import { counts, layersOf, STATUSES } from "../../lib/data/holding";
    import { PARTS, type SourceRow } from "../../lib/data/sources";

    // Each product and layer of `used_by` with a chip for each source that it reads, in the colour
    // of its status, as `obc data sources --layer` lists them. A prepared source stands for the
    // sources that only its prepare reads.
    let { rows }: { rows: SourceRow[] } = $props();

    const layers = $derived(layersOf(rows));
</script>

<p class="soft">Whether a bake still downloads depends on its region: By region tells.</p>
<div class="layers">
    {#each layers as { layer, rows: read } (layer)}
        <section class="card" aria-labelledby="layer-{layer}">
            <div class="title">
                <h2 id="layer-{layer}">{PARTS[layer] ?? layer}</h2>
                <span class="soft">{counts(read).filter(([, n]) => n).map(([status, n]) => `${n} ${STATUSES[status].toLowerCase()}`).join(" · ")}</span>
            </div>
            <ul class="chips">
                {#each read as row (row.id)}
                    <li><a class="pill" data-status={row.held.status} href="#/sources/{encodeURIComponent(row.id)}" title="{row.use ?? row.id} · {STATUSES[row.held.status]}">{row.id}</a></li>
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
