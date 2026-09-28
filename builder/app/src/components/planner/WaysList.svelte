<script lang="ts" module>
    import type { Trip } from '../../lib/planner/editor';

    export const ways: { id: Trip['variant']; title: string; description: string }[] = [
        { id: 'valley', title: 'Along the valley', description: 'Canal and Doubs corridor' },
        { id: 'direct', title: 'More direct', description: 'Fewer bends' },
    ];
</script>

<script lang="ts">
    import { cumulative, routeCoordinates } from '../../lib/planner/editor';
    import { profileAscent } from '../../lib/planner/profile-data';

    let { trip, onPick }: { trip: Trip; onPick: (variant: Trip['variant']) => void } = $props();

    const lengths = $derived(ways.map(way => cumulative(routeCoordinates({ ...trip, variant: way.id })).at(-1)!));
</script>

<div class="ways" role="radiogroup" aria-label="Ways">
    {#each ways as way, index (way.id)}
        <button type="button" role="radio" aria-checked={trip.variant === way.id} onclick={() => onPick(way.id)}>
            <span class="mark"></span>
            <span class="what"><strong>{way.title}</strong><small>{way.description}</small></span>
            <span class="figure">{lengths[index].toFixed(1)} km<small>↑ {profileAscent()} m</small></span>
        </button>
    {/each}
    <p class="note">Example geometry · pinned places stay fixed</p>
</div>

<style>
    .ways {
        padding: 4px 16px 16px;
    }
    button {
        display: flex;
        align-items: center;
        gap: 12px;
        width: calc(100% + 16px);
        margin: 0 -8px 4px;
        padding: 12px 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
        text-align: left;
        font: inherit;
        cursor: pointer;
    }
    button:hover {
        background: var(--parchment-2);
    }
    .mark {
        width: 16px;
        height: 16px;
        flex: none;
        border: 1px solid var(--line-strong);
        border-radius: 50%;
    }
    [aria-checked="true"] .mark {
        border: 5px solid var(--ink);
    }
    .what {
        flex: 1;
        min-width: 0;
    }
    strong,
    small {
        display: block;
    }
    strong {
        font: 600 14px var(--sans);
    }
    small {
        margin-top: 2px;
        font: 400 13px var(--sans);
        color: var(--ink-soft);
    }
    .figure {
        text-align: right;
        font: 700 17px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .note {
        margin: 8px 0 0;
        font-size: 13px;
        color: var(--ink-soft);
    }
</style>
