<!-- The left card of the page of a long trail: the kind line, the totals, the description, a row for each stage and the
     credits. A row opens the page of its stage; the pointer on a row lights that stage. -->
<script lang="ts">
    import { duration } from '../../lib/planner/editor';
    import { catalog } from '../../lib/planner/route-catalog';
    import { gradeName, type CatalogRecord } from '../../lib/planner/signed-routes';
    import { formatDistance, formatHeight } from '../../lib/settings/format';
    import Card from '../../lib/ui/Card.svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import { kindName } from './SignedCard.svelte';
    import { shownDescription } from './signed-page';
    import { heldStages } from './strip';
    import type { TrailRow } from './trail-page';

    let { record, rows, hovered, onhover }: {
        record: CatalogRecord;
        rows: TrailRow[];
        /** The id of the lit part. */
        hovered?: number;
        onhover: (id: number | undefined) => void;
    } = $props();

    const lead = $derived([record.loop ? 'Loop' : 'One way', kindName(record),
        ...record.kind !== 'bicycle' && record.hardest !== undefined ? [gradeName(record.kind, record.hardest)] : []].join(' · '));
    // As the count in the title: a trail of fewer than two stages shows none.
    const stages = $derived.by(() => {
        const count = record.stage_count ?? 0, held = heldStages(record.parts ?? []);
        return count < 2 ? undefined : held < count ? `${held} of ${count}` : String(count);
    });
    const description = $derived(shownDescription(record, id => catalog.record(id)));
</script>

{#snippet number(label: string, value: string)}
    <div><dt>{label}</dt><dd>{value}</dd></div>
{/snippet}

<div class="facts">
    <Card>
        <p class="lead">{lead}</p>
        <dl class="numbers">
            {@render number('Length', formatDistance(record.length_m))}
            {@render number('Climb', formatHeight(record.ascent_m))}
            {#if record.moving_s !== undefined}{@render number('Time', duration(record.moving_s / 3600))}{/if}
            {#if stages}{@render number('Stages', stages)}{/if}
        </dl>
        {#if description}<p class="description">{description}</p>{/if}
        <h3>Stages</h3>
        <ul>
            {#each rows as row (row.id)}
                <li class:lit={row.id === hovered} class:gap={row.gap} onpointerenter={() => onhover(row.id)} onpointerleave={() => onhover(undefined)}>
                    {#if row.href}
                        <a href={row.href}><span><strong>{row.label}</strong><small>{row.numbers.join(' · ')}</small></span><Icon name="chevron" size={14} /></a>
                    {:else}
                        <span><strong>{row.label}</strong><small>{row.numbers.join(' · ')}</small></span>
                    {/if}
                </li>
            {/each}
        </ul>
        <p class="foot">
            {#if record.operator}<span>{record.operator}</span>{/if}
            {#if record.credit}<span>Route data: {record.credit}</span>{/if}
            <span>Route from OpenStreetMap</span>
        </p>
    </Card>
</div>

<style>
    /* As the left card of the Route page: as high as the frame gives it, and it scrolls inside itself. */
    .facts {
        min-height: 0;
    }
    .facts > :global(section) {
        display: flex;
        flex-direction: column;
        height: 100%;
        overflow: auto;
    }
    .lead {
        margin: 0 0 12px;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .numbers {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 14px 12px;
        margin: 0;
    }
    dt, h3 {
        color: var(--ink-soft);
        font: 13px/1.3 var(--sans);
    }
    dd {
        margin: 1px 0 0;
        font: 600 21px/1.2 var(--sans);
    }
    .description {
        margin: 14px 0 0;
    }
    h3 {
        margin: 20px 0 0;
        padding-bottom: 6px;
        border-bottom: 1px solid var(--line);
        font-weight: 600;
    }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        border-bottom: 1px solid var(--line);
    }
    a {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        color: var(--ink);
        text-decoration: none;
    }
    a, li > span {
        padding: 8px 6px;
    }
    a > span, li > span {
        display: flex;
        flex-direction: column;
        gap: 2px;
        min-width: 0;
    }
    .lit {
        background: var(--parchment-2);
    }
    a:hover strong {
        text-decoration: underline;
    }
    small {
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .gap {
        color: var(--ink-faint);
    }
    .gap strong {
        font-weight: 400;
    }
    .gap small {
        color: inherit;
    }
    .foot {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 12px;
        margin: auto 0 0;
        padding-top: 20px;
        color: var(--ink-soft);
        font-size: 12.5px;
    }
</style>
