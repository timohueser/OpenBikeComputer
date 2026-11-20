<!-- The wishes, in two groups: on the way, and the route. A tick makes a wish nice, which orders the hikes; Must keeps
     only the hikes that meet it. Each wish counts the hikes inside the limits that meet it. A wish with no data for the
     loaded hikes is not offered; "Official route" shows only while a made route is loaded. -->
<script lang="ts">
    import type { FactsPlace } from '../../lib/planner/signed-routes';
    import Icon from '../../lib/ui/Icon.svelte';
    import { wishesOf, type Query, type Wish } from './search';
    import { ROUTE_WISHES, WAY_WISHES, wishIcon, wishLabel } from './words';

    let { query, counts, place, onchange }: {
        query: Query;
        /** The count of each wish; null with no data. */
        counts: Map<Wish, number | null>;
        place: (id: string) => FactsPlace | undefined;
        onchange: (change: Partial<Query>) => void;
    } = $props();

    const passes = $derived(wishesOf(query).filter(wish => wish.startsWith('via:')));

    function set(wish: Wish, level: 'nice' | 'must' | undefined) {
        const wishes = { ...query.wishes };
        if (level) wishes[wish] = level;
        else delete wishes[wish];
        onchange({ wishes });
    }
</script>

<section>
    <div class="head"><h3>On the way</h3></div>
    <p class="hint">Ticked wishes put the hikes that have them first. Must hides the others.</p>
    <div class="wishes">
        {#each [...passes, ...WAY_WISHES] as wish (wish)}{@render row(wish)}{/each}
    </div>
    <p class="hint"><Icon name="pin" size={14} />Or click a summit, hut or lake on the map.</p>
</section>
<section>
    <div class="head"><h3>The route</h3></div>
    <div class="wishes">
        {#each ROUTE_WISHES.filter(wish => wish !== 'official' || typeof counts.get(wish) === 'number' || query.wishes.official) as wish (wish)}{@render row(wish)}{/each}
    </div>
</section>

{#snippet row(wish: Wish)}
    {@const level = query.wishes[wish]}
    {@const count = counts.get(wish)}
    {@const none = count === null && !level}
    <div class="wish" class:on={!!level}>
        <button type="button" class="pick" aria-pressed={!!level} disabled={none} onclick={() => set(wish, level ? undefined : 'nice')}>
            <span class="box"><Icon name="check" size={12} /></span><span class="icon"><Icon name={wishIcon(wish, place)} /></span><span class="label">{wishLabel(wish, place)}</span>
        </button>
        {#if level}
            <button type="button" class="must" aria-pressed={level === 'must'} title="Only show hikes that have this" onclick={() => set(wish, level === 'must' ? 'nice' : 'must')}>
                <Icon name="lock" size={12} />Must
            </button>
        {/if}
        <span class="count" title={count === null ? 'No data for the hikes here' : undefined}>{count === null ? 'no data' : count}</span>
    </div>
{/snippet}

<style>
    .wishes {
        display: flex;
        flex-direction: column;
        margin: 0 -8px;
    }
    .wish {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto minmax(26px, auto);
        align-items: center;
        gap: 8px;
        padding: 0 8px;
        border-radius: 8px;
    }
    .wish:hover {
        background: var(--parchment);
    }
    .pick {
        display: flex;
        grid-column: 1;
        align-items: center;
        gap: 10px;
        min-width: 0;
        min-height: 36px;
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink);
        font: 13.5px var(--sans);
        text-align: left;
    }
    .pick:disabled {
        color: var(--ink-soft);
    }
    .box {
        display: grid;
        flex: none;
        place-items: center;
        width: 18px;
        height: 18px;
        border: 1.5px solid var(--line-strong);
        border-radius: 5px;
        background: var(--panel);
        color: var(--panel);
    }
    .box :global(svg) {
        transform: scale(0);
        transition: transform 180ms cubic-bezier(0.23, 1, 0.32, 1);
        stroke-width: 3;
    }
    .pick:hover:not(:disabled) .box {
        border-color: var(--ink-soft);
    }
    [aria-pressed="true"] > .box {
        border-color: var(--ink);
        background: var(--ink);
    }
    [aria-pressed="true"] > .box :global(svg) {
        transform: scale(1);
    }
    .icon {
        display: flex;
        color: var(--ink-soft);
    }
    .on .icon {
        color: var(--ink);
    }
    .label {
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .on .label {
        font-weight: 600;
    }
    .must {
        display: inline-flex;
        grid-column: 2;
        align-items: center;
        gap: 4px;
        height: 24px;
        padding: 0 9px 0 7px;
        border: 1px solid var(--line-strong);
        border-radius: 12px;
        background: var(--panel);
        color: var(--ink-soft);
        font: 600 12px var(--sans);
    }
    .must:hover {
        border-color: var(--ink);
        color: var(--ink);
    }
    /* Rust is the one colour of a hard limit. */
    .must[aria-pressed="true"] {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
    }
    .count {
        grid-column: 3;
        color: var(--ink-soft);
        font-size: 12.5px;
        font-variant-numeric: tabular-nums;
        text-align: right;
        white-space: nowrap;
    }
</style>
