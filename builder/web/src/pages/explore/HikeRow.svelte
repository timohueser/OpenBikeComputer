<!-- A row of the day-hike search: one route in one shape, with the numbers of that shape, the way there, and each nice
     wish, with a tick when the row meets it. -->
<script lang="ts">
    import type { FactsPlace } from '../../lib/planner/signed-routes';
    import Icon from '../../lib/ui/Icon.svelte';
    import type { SignedItem } from '../routes/filter';
    import RouteRow from '../routes/RouteRow.svelte';
    import type { Row, Wish } from './search';
    import SignedPicture from './SignedPicture.svelte';
    import { accessSteps, hikeNumbers, SHAPE_WORDS, wishLabel } from './words';

    let { row, item, nice, place, current, onclick, oncover }: {
        row: Row;
        /** The item of the route of the row. */
        item: SignedItem;
        /** The nice wishes of the search. */
        nice: Wish[];
        place: (id: string) => FactsPlace | undefined;
        current: boolean;
        onclick: () => void;
        oncover: (credits: string[] | undefined) => void;
    } = $props();

    const steps = $derived(accessSteps(row, place));
</script>

<RouteRow name={item.name} {current} line={item.id} {onclick} parts={[SHAPE_WORDS[row.shape].tag, ...hikeNumbers(row)]}
    badge={'made' in row.record ? { text: 'Ours', kind: 'ours' } : undefined}>
    {#snippet picture()}<SignedPicture {item} {oncover} row />{/snippet}
    {#if steps.length}
        <span class="line">
            {#each steps as step, i (i)}
                <span class="step" class:plain={!step.icon && !step.park}>{#if step.park}<span class="park" aria-hidden="true">P</span>{:else if step.icon}<Icon name={step.icon} size={14} />{/if}{step.text}</span>
            {/each}
        </span>
    {/if}
    {#if nice.length}
        <span class="line ticks">
            {#each nice as wish (wish)}
                {@const met = row.met.includes(wish)}
                {@const unknown = row.unknown.includes(wish)}
                <span class:met>
                    {#if met}<Icon name="check" size={13} />{:else if unknown}<span aria-hidden="true">?</span>{:else}<Icon name="close" size={11} /><span class="hidden">{'no '}</span>{/if}{wishLabel(wish, place, true)}{#if unknown}<span class="hidden">{': no data'}</span>{/if}
                </span>
            {/each}
        </span>
    {/if}
</RouteRow>

<style>
    .line {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 2px 4px;
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .step, .ticks span {
        display: inline-flex;
        align-items: center;
        gap: 3px;
        white-space: nowrap;
    }
    .step {
        color: var(--ink);
    }
    .step + .step::before {
        content: "›";
        margin-right: 1px;
        color: var(--ink-soft);
    }
    .step :global(svg), .plain {
        color: var(--ink-soft);
    }
    .park {
        display: inline-grid;
        flex: none;
        place-items: center;
        width: 15px;
        height: 15px;
        border-radius: 4px;
        background: var(--water);
        color: var(--panel);
        font: 700 10px/1 var(--sans);
    }
    .ticks {
        gap: 2px 10px;
    }
    /* A wish that the row misses or has no data for is faint. */
    .ticks span:not(.met) {
        color: var(--ink-faint);
    }
    .ticks .met {
        color: var(--ink);
    }
    .hidden {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
    }
</style>
