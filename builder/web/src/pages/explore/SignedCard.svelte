<!-- The card of the selected signed route over the map: its line, its mark and name, what kind of route it is, its
     numbers and profile, its description and official page, the credit of its source, and "Plan this route". -->
<script module lang="ts">
    import { gradeName, longTrailId, type CatalogRecord } from '../../lib/planner/signed-routes';
    import { formatDistance, formatHeight } from '../../lib/settings/format';

    type About = Pick<CatalogRecord, 'kind' | 'rank' | 'hardest' | 'length_m' | 'ascent_m'>;

    /** The numbers of a row and of the card: the length, the climb, and the hardest grade of a graded route. */
    export function signedNumbers(record: About): string[] {
        const grade = record.kind !== 'bicycle' && record.hardest !== undefined ? [gradeName(record.kind, record.hardest)] : [];
        return [formatDistance(record.length_m), `${formatHeight(record.ascent_m)} up`, ...grade];
    }

    const LEVELS = ['', 'Local', 'Regional', 'National', 'International'];
    const KINDS: Record<CatalogRecord['kind'], string> = { hiking: 'hiking route', foot: 'hiking route', bicycle: 'cycling route', mtb: 'mountain bike route' };

    /** "National hiking route", or "Hiking route" with no network level. */
    export const kindName = (record: About) =>
        LEVELS[record.rank] ? `${LEVELS[record.rank]} ${KINDS[record.kind]}` : KINDS[record.kind].replace(/^./, first => first.toUpperCase());
</script>

<script lang="ts">
    import TrailMarker from '../../components/planner/TrailMarker.svelte';
    import { catalog } from '../../lib/planner/route-catalog';
    import { routeWebsite } from '../../lib/planner/route-overlays';
    import type { SignedItem } from '../routes/filter';
    import RouteCard from '../routes/RouteCard.svelte';
    import SignedPicture from './SignedPicture.svelte';

    let { item, oncover, onplan, ontrail, onclose }: {
        item: SignedItem;
        /** As `oncover` of the picture: it is too small for its credits, so the page shows them with the map. */
        oncover?: (credits: string[] | undefined) => void;
        onplan: () => void;
        /** With it, the name of the long route of a stage or a section is a button that opens the card of its trail. */
        ontrail?: (id: number) => void;
        onclose: () => void;
    } = $props();

    const record = $derived(item.record);
    const recordOf = (id: number) => catalog.record(id);
    /** The stage is in a trail of the view only through its complete long route (`parent`): the card names that trail. */
    const byParent = $derived(item.trail !== undefined && Number(item.trail) === record.parent && record.parent !== longTrailId(record, recordOf));
    /** The long route of a stage or a section: each file that holds a part also holds its long route. */
    const longId = $derived(byParent ? record.parent : record.trail ?? record.parent), long = $derived(longId === undefined ? undefined : recordOf(longId));
    // `parent` and `trail` can name two long routes: the number must be of the one that the card names.
    const number = $derived(record.trail !== undefined && !byParent ? record.trail_stage : record.stage), of = $derived(long?.stages?.length ?? long?.stage_count);
    const part = $derived(record.section !== undefined ? `Section ${record.section}` : number === undefined ? false : of ? `Stage ${number} of ${of}` : `Stage ${number}`);
    // A cut stage is no trail: its sections open the trail that the stage is in. A cut route of fewer than two stages has
    // a trail record file only when it takes more than 12 hours, which no reader knows: its name is no button.
    const longName = $derived((part && (long?.name ?? long?.ref)) || false);
    const opens = $derived(!!ontrail && !!long && (byParent || long.trail !== undefined || (long.stage_count ?? 0) >= 2));
    const trailName = $derived(longName && opens ? { text: longName, onclick: () => ontrail!(byParent ? record.parent! : longTrailId(record, recordOf)!) } : longName);
    const website = $derived(routeWebsite(record.website));
    const about = $derived(!!(record.description || record.operator || website || record.credit));
</script>

<RouteCard name={item.name} meta={[record.loop ? 'Loop' : 'One way', part, trailName, kindName(record)]} numbers={signedNumbers(record)} profile={item.profile}
    more={about ? more : undefined} {onclose}>
    {#snippet picture()}<SignedPicture {item} {oncover} />{/snippet}
    {#snippet mark()}<span class="mark"><TrailMarker symbol={record.symbol} ref={record.ref} /></span>{/snippet}
    {#snippet buttons()}<button type="button" class="button primary" onclick={onplan}>Plan this route</button>{/snippet}
</RouteCard>

{#snippet more()}
    <p class="about">
        {#if record.description}<span class="description">{record.description}</span>{/if}
        {#if record.operator}<span>{record.operator}</span>{/if}
        {#if website}<a href={website} target="_blank" rel="noopener noreferrer">Official page ↗</a>{/if}
        {#if record.credit}<span>Route data: {record.credit}</span>{/if}
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
