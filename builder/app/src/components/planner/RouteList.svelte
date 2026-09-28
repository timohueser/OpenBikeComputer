<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { profileAscent } from '../../lib/planner/profile-data';
    import type { RoutePoint } from '../../lib/planner/editor';

    let { stops, onInspect, onReorder }: {
        stops: { point: RoutePoint; distance: number }[];
        onInspect: (point: RoutePoint) => void;
        onReorder: (id: string, direction: -1 | 1) => void;
    } = $props();

    const total = $derived(stops.at(-1)?.distance || 1);
</script>

<ol class="route">
    {#each stops as { point, distance }, index (point.id)}
        <li>
            <button type="button" class="stop" onclick={() => onInspect(point)}>
                <Icon name={point.kind === 'via' ? 'route' : point.kind === 'start' || point.kind === 'finish' ? 'pin' : 'flag'} size={17} />
                <span>
                    <strong>{point.kind === 'via' ? 'Shaping point' : point.label}</strong>
                    <small>{distance.toFixed(1)} km · ↑ {profileAscent(0, distance / total)} m</small>
                </span>
            </button>
            {#if index > 0 && index < stops.length - 1}
                <span class="reorder">
                    <button type="button" aria-label={`Move ${point.label} earlier`} disabled={index === 1} onclick={() => onReorder(point.id, -1)}><Icon name="up" size={15} /></button>
                    <button type="button" aria-label={`Move ${point.label} later`} disabled={index === stops.length - 2} onclick={() => onReorder(point.id, 1)}><Icon name="down" size={15} /></button>
                </span>
            {:else}
                <small class="end">{index === 0 ? 'Start' : 'Finish'}</small>
            {/if}
        </li>
    {/each}
</ol>
<p class="note">Cumulative from the start · climb is illustrative</p>

<style>
    .route {
        margin: 0;
        padding: 4px 16px 0;
        list-style: none;
    }
    li {
        display: flex;
        align-items: center;
        gap: 8px;
        margin-bottom: 4px;
    }
    button {
        border: 0;
        background: none;
        color: var(--ink);
        font: inherit;
        cursor: pointer;
    }
    .stop {
        flex: 1;
        min-width: 0;
        display: flex;
        align-items: center;
        gap: 12px;
        margin-left: -8px;
        padding: 8px;
        border-radius: 6px;
        text-align: left;
    }
    .stop:hover {
        background: var(--parchment-2);
    }
    .stop > :global(svg) {
        flex: none;
        color: var(--ink-soft);
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
        font-size: 13px;
        color: var(--ink-soft);
        font-variant-numeric: tabular-nums;
    }
    .end {
        margin: 0;
    }
    .reorder {
        display: flex;
    }
    .reorder button {
        display: grid;
        place-items: center;
        width: 30px;
        height: 30px;
        border-radius: 6px;
    }
    .reorder button:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    .note {
        margin: 8px 16px 16px;
        font-size: 13px;
        color: var(--ink-soft);
    }
</style>
