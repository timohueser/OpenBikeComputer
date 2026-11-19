<script lang="ts">
    import type { Surface } from '../../lib/planner/routing';
    import { surfaceBarColors, type SurfaceShare } from '../../lib/planner/surface-data';

    /** The surface shares of a stretch as one line: the colour, the name and the percent of each surface. `lit` is the
     * surface of the entry under the pointer or with the focus, for the bar to light. `current` marks an entry while
     * no entry is lit. A stretch that is all Unknown has no surface data, and the line says so. */
    let { shares, lit = $bindable(), current }: {
        shares: SurfaceShare[];
        lit?: Surface;
        current?: Surface;
    } = $props();
    const marked = $derived(lit ?? current);
    const known = $derived(shares.some(share => share.surface !== 'Unknown'));
    // An entry that leaves the list while it is lit gets no `pointerleave`.
    $effect(() => { if (lit && !(known && shares.some(share => share.surface === lit))) lit = undefined; });
</script>

{#if known}
    <ul aria-label="Surface shares">
        {#each shares as { surface, percent } (surface)}
            <!-- An entry takes the focus only to light its surface, and it has no action: as a list item, a screen
                 reader announces its text and no control. -->
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <li tabindex="0" class:unknown={surface === 'Unknown'} class:marked={surface === marked} class:dim={marked && surface !== marked}
                onpointerenter={() => lit = surface} onpointerleave={() => lit = undefined} onfocus={() => lit = surface} onblur={() => lit = undefined}>
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
        border-radius: 2px;
        white-space: nowrap;
    }
    li:focus-visible {
        outline: 2px solid var(--ink);
        outline-offset: 2px;
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
    .unknown {
        color: var(--ink-faint);
    }
    .unknown b {
        color: inherit;
        font-weight: 400;
    }
    .marked {
        color: var(--ink);
    }
    .dim {
        opacity: .45;
    }
</style>
