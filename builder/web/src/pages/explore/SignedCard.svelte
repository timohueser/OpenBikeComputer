<!-- The card of the selected signed route over the map: its line, its mark and name, what kind of route it is, its
     numbers and profile, its description and official page, the credit of its source, and "Plan this route". For a row of
     the day-hike search, the numbers are those of its shape, and the facts of the row show over the description. -->
<script module lang="ts">
    import { gradeName, type CatalogRecord } from '../../lib/planner/signed-routes';
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
    import HikeFacts from './HikeFacts.svelte';
    import type { Row } from './search';
    import SignedPicture from './SignedPicture.svelte';
    import { shownDescription, stagePart } from './signed-page';
    import { hikeNumbers } from './words';

    let { item, row, href, oncover, onplan, ontrail, onclose }: {
        item: SignedItem;
        /** The row of the day-hike search that the card shows: the route in one shape. */
        row?: Row;
        /** The page of the route: the name links to it. */
        href?: string;
        /** As `oncover` of the picture: it is too small for its credits, so the page shows them with the map. */
        oncover?: (credits: string[] | undefined) => void;
        onplan: () => void;
        /** With it, the name of the long route of a stage or a section is a button that opens the card of its trail. */
        ontrail?: (id: number) => void;
        onclose: () => void;
    } = $props();

    const record = $derived(item.record);
    const recordOf = (id: number) => catalog.record(id);
    /** The long route of a stage or a section: each file that holds a part also holds its long route. */
    const { part, long, byParent, trailId } = $derived(stagePart(record, recordOf));
    // A cut stage is no trail: its sections open the trail that the stage is in. A cut route of fewer than two stages has
    // a trail record file only when it takes more than 12 hours, which no reader knows: its name is no button.
    const longName = $derived((part && (long?.name ?? long?.ref)) || false);
    const opens = $derived(!!ontrail && !!long && (byParent || long.trail !== undefined || (long.stage_count ?? 0) >= 2));
    const trailName = $derived(longName && opens ? { text: longName, onclick: () => ontrail!(trailId!) } : longName);
    const website = $derived(routeWebsite(record.website));
    const description = $derived(shownDescription(record, recordOf));
    const about = $derived(!!(description || record.operator || website || record.credit));
    // A row has the word of a loop only: another shape of a row has its sentence in the facts.
    const shape = $derived(row ? row.shape === 'loop' && 'Loop' : record.loop ? 'Loop' : 'One way');
    const kind = $derived('made' in record ? `Our ${record.loop ? 'loop' : 'route'}, made from signed hiking paths` : kindName(record));
</script>

<RouteCard name={item.name} {href} meta={[shape, part, trailName, kind]} numbers={row ? hikeNumbers(row, 'Grade unknown') : signedNumbers(record)}
    profile={item.profile} more={about || row ? more : undefined} {onclose}>
    {#snippet picture()}<SignedPicture {item} {oncover} />{/snippet}
    {#snippet mark()}<TrailMarker symbol={record.symbol} ref={record.ref} />{/snippet}
    {#snippet buttons()}<button type="button" class="button primary" onclick={onplan}>Plan this route</button>{/snippet}
</RouteCard>

{#snippet more()}
    {#if row}<HikeFacts {row} highest={item.summary.highestM} place={id => catalog.place(id)} />{/if}
    {#if about}
        <p class="about">
            {#if description}<span class="description">{description}</span>{/if}
            {#if record.operator}<span>{record.operator}</span>{/if}
            {#if website}<a href={website} target="_blank" rel="noopener noreferrer">Official page ↗</a>{/if}
            {#if record.credit}<span>Route data: {record.credit}</span>{/if}
        </p>
    {/if}
{/snippet}

<style>
    .about {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 14px;
        margin: 0;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .about:not(:first-child) {
        margin-top: 10px;
    }
    .description {
        flex-basis: 100%;
        color: var(--ink);
    }
    a {
        color: var(--ink);
    }
</style>
