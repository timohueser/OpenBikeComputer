<!-- The card of the selected signed route over the map: its line, its mark and name, what kind of route it is, its
     numbers and profile, its description and official page, and "Plan this route". -->
<script module lang="ts">
    import { gradeName, type CatalogRecord } from '../../lib/planner/signed-routes';
    import { formatDistance, formatHeight } from '../../lib/settings/format';

    /** The numbers of a row and of the card: the length, the climb, and the hardest grade of a graded route. */
    export function signedNumbers(record: CatalogRecord): string[] {
        const grade = record.kind !== 'bicycle' && record.hardest !== undefined ? [gradeName(record.kind, record.hardest)] : [];
        return [formatDistance(record.length_m), `${formatHeight(record.ascent_m)} up`, ...grade];
    }
</script>

<script lang="ts">
    import TrailMarker from '../../components/planner/TrailMarker.svelte';
    import { catalog } from '../../lib/planner/route-catalog';
    import { routeWebsite } from '../../lib/planner/route-overlays';
    import type { SignedItem } from '../routes/filter';
    import RouteCard from '../routes/RouteCard.svelte';
    import SignedPicture from './SignedPicture.svelte';

    let { item, onplan }: { item: SignedItem; onplan: () => void } = $props();

    const LEVELS = ['', 'Local', 'Regional', 'National', 'International'];
    const KINDS: Record<CatalogRecord['kind'], string> = { hiking: 'hiking route', foot: 'hiking route', bicycle: 'cycling route', mtb: 'mountain bike route' };

    const record = $derived(item.record);
    /** The long route of a stage: each file that holds a stage also holds its long route. */
    const parent = $derived(record.parent === undefined ? undefined : catalog.record(record.parent));
    const kind = $derived(LEVELS[record.rank] ? `${LEVELS[record.rank]} ${KINDS[record.kind]}` : KINDS[record.kind].replace(/^./, first => first.toUpperCase()));
    const stage = $derived(record.stage === undefined ? false
        : parent?.stages ? `Stage ${record.stage} of ${parent.stages.length} · ${parent.name ?? parent.ref}` : `Stage ${record.stage}`);
    const website = $derived(routeWebsite(record.website));
    const about = $derived(!!(record.description || record.operator || website));
</script>

<RouteCard name={item.name} meta={[record.loop ? 'Loop' : 'One way', stage, kind]} numbers={signedNumbers(record)} profile={item.profile}
    more={about ? more : undefined}>
    {#snippet picture()}<SignedPicture {item} />{/snippet}
    {#snippet mark()}<span class="mark"><TrailMarker symbol={record.symbol} ref={record.ref} /></span>{/snippet}
    {#snippet buttons()}<button type="button" class="button" onclick={onplan}>Plan this route</button>{/snippet}
</RouteCard>

{#snippet more()}
    <p class="about">
        {#if record.description}<span class="description">{record.description}</span>{/if}
        {#if record.operator}<span>{record.operator}</span>{/if}
        {#if website}<a href={website} target="_blank" rel="noopener noreferrer">Official page ↗</a>{/if}
    </p>
{/snippet}

<style>
    /* The mark stands on the first line of the name. */
    .mark {
        display: inline-block;
        margin-right: 8px;
        vertical-align: -5px;
    }
    .about {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 14px;
        margin: 0;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .description {
        flex-basis: 100%;
        color: var(--ink);
    }
    a {
        color: var(--ink);
    }
</style>
