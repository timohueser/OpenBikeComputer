<script lang="ts">
    import type { Surface } from '../../lib/planner/routing';
    import { surfaceBarColors, type SurfaceShare } from '../../lib/planner/surface-data';

    /** The surface shares of a route as one line: the colour, the name and the percent of each surface. `lit` is the
     * surface of the entry under the pointer, for the bar to light. `current` marks an entry while no entry is under
     * the pointer. A route whose every stretch is Unknown has no surface data, and the line says so. */
    let { shares, lit = $bindable(), current }: {
        shares: SurfaceShare[];
        lit?: Surface;
        current?: Surface;
    } = $props();
    const marked = $derived(lit ?? current);
    const known = $derived(shares.some(share => share.surface !== 'Unknown'));
    // An entry that leaves the list under the pointer gets no `pointerleave`.
    $effect(() => { if (lit && !(known && shares.some(share => share.surface === lit))) lit = undefined; });
</script>

{#if known}
    <ul aria-label="Surface shares">
        {#each shares as { surface, percent } (surface)}
            <li class:marked={surface === marked} class:dim={marked && surface !== marked} onpointerenter={() => lit = surface} onpointerleave={() => lit = undefined}>
                <i style:background={surfaceBarColors[surface]}></i>{surface} <b>{percent || '<1'}%</b>
            </li>
        {/each}
    </ul>
{:else}
    <span>No surface data</span>
{/if}

<style>
    span {
        color: var(--ink-soft);
        font: 12px/16px var(--sans);
    }
    ul {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 14px;
        margin: 0;
        padding: 0;
        list-style: none;
        color: var(--ink-soft);
        font: 12px/16px var(--sans);
    }
    li {
        display: flex;
        align-items: center;
        gap: 5px;
        white-space: nowrap;
    }
    i {
        width: 9px;
        height: 9px;
        border-radius: 2px;
    }
    b {
        color: var(--ink);
        font-weight: 600;
        font-variant-numeric: tabular-nums;
    }
    .marked {
        color: var(--ink);
    }
    .dim {
        opacity: .45;
    }
</style>
