<!-- The facts of a row of the day-hike search, for its card: the shape and the way there in words, the wishes that it meets
     and misses, and the facts of the route. -->
<script lang="ts">
    import type { FactsPlace } from '../../lib/planner/signed-routes';
    import { formatHeight } from '../../lib/settings/format';
    import Icon from '../../lib/ui/Icon.svelte';
    import type { Row } from './search';
    import { accessText, placeName, shapeText, snowText, wishLabel } from './words';

    let { row, highest, place }: {
        row: Row;
        /** The highest point of the route in metres, if known. */
        highest: number | null;
        place: (id: string) => FactsPlace | undefined;
    } = $props();

    const how = $derived(accessText(row, place));
    const lines = $derived.by(() => {
        const facts = row.facts, snow = snowText(facts?.snow_free);
        const names = (facts?.places ?? []).map(([id]) => placeName(place(id)));
        const passes = names.length > 5 ? `${names.slice(0, 5).join(', ')} and ${names.length - 5} more` : names.join(', ');
        return [
            highest !== null && { icon: 'mountain', text: `Highest point ${formatHeight(highest)}` },
            snow && { icon: 'snow', text: snow },
            facts && { icon: 'paved', text: `${facts.paved_pct} % paved` },
            facts?.canopy_pct !== undefined && { icon: 'tree', text: `${facts.canopy_pct} % forest` },
            facts && facts.big_road_pct > 5 && { icon: 'quiet', text: `${facts.big_road_pct} % near big roads` },
            passes && { icon: 'pin', text: `Passes ${passes}` },
        ].filter(line => !!line);
    });
</script>

<div class="hike">
    <div class="words">
        <p>{shapeText(row)}</p>
        {#if how}<p>{how}</p>{/if}
        {#if row.met.length + row.missed.length + row.unknown.length}
            <p class="tags">
                {#each row.met as wish (wish)}<span><Icon name="check" size={13} />{wishLabel(wish, place, true)}</span>{/each}
                {#each row.missed as wish (wish)}<span class="no"><Icon name="close" size={11} /><span class="hidden">{'no '}</span>{wishLabel(wish, place, true)}</span>{/each}
                {#each row.unknown as wish (wish)}<span class="no">?{wishLabel(wish, place, true)}</span>{/each}
            </p>
        {/if}
    </div>
    {#if lines.length}
        <ul>
            {#each lines as line (line.icon)}<li><Icon name={line.icon} size={16} />{line.text}</li>{/each}
        </ul>
    {/if}
</div>

<style>
    .hike {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
        gap: 8px 20px;
        font-size: 13px;
    }
    .words {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }
    p {
        margin: 0;
    }
    .tags {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 12px;
    }
    .tags span {
        display: inline-flex;
        align-items: center;
        gap: 3px;
        white-space: nowrap;
    }
    .tags .no {
        color: var(--ink-faint);
    }
    .hidden {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
    }
    ul {
        display: flex;
        flex-direction: column;
        gap: 4px;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        display: flex;
        align-items: center;
        gap: 8px;
    }
    li :global(svg) {
        flex: none;
        color: var(--ink-soft);
    }
</style>
